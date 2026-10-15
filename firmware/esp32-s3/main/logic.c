// Generic WiFi messages and proof algorithm ported from gbs-anywhere PR15/16.
#include "logic.h"
#include <ctype.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

bool gbs_reply_valid(int http_status, const char *status)
{
    if (!status || !status[0]) return false;
    // Preserve named API errors on HTTP 4xx so the worker can recover expired jobs.
    return (http_status >= 200 && http_status < 300) ||
        (http_status >= 400 && http_status < 500 && strcmp(status, "Ok") != 0);
}

bool gbs_uuid_valid(const char *s)
{
    if (!s || strlen(s) != 36) return false;
    for (int i = 0; i < 36; ++i) {
        if (i == 8 || i == 13 || i == 18 || i == 23) {
            if (s[i] != '-') return false;
        } else if (!isxdigit((unsigned char)s[i])) return false;
    }
    return true;
}

static double number(const cJSON *j, const char *key)
{
    const cJSON *n = cJSON_GetObjectItemCaseSensitive(j, key);
    return cJSON_IsNumber(n) ? n->valuedouble : NAN;
}

bool gbs_parse_job(const cJSON *j, gbs_job_t *job)
{
    const cJSON *id = cJSON_GetObjectItemCaseSensitive(j, "grindUuid");
    if (!cJSON_IsString(id) || !gbs_uuid_valid(id->valuestring)) return false;
    memset(job, 0, sizeof(*job));
    memcpy(job->uuid, id->valuestring, sizeof(job->uuid));
    const cJSON *group = cJSON_GetObjectItemCaseSensitive(j, "groupHeadId");
    if (group && !cJSON_IsNull(group)) {
        double g = number(j, "groupHeadId");
        if (!isfinite(g) || g < 0 || g > UINT32_MAX || floor(g) != g) return false;
        job->group = (uint32_t)g;
    }
    const cJSON *dose = cJSON_GetObjectItemCaseSensitive(j, "doseTypeExpected");
    if (dose) {
        char *encoded = cJSON_PrintUnformatted(dose);
        if (!encoded) return false;
        size_t len = strlen(encoded);
        if (len < sizeof(job->dose_json)) memcpy(job->dose_json, encoded, len + 1);
        cJSON_free(encoded);
        if (len >= sizeof(job->dose_json)) return false;
    } else strcpy(job->dose_json, "null");
    double ms = number(j, "durationExpectedMs"), mg = number(j, "brewWeightExpectedMg");
    if (isfinite(ms) && ms > 0) job->target_ms = (uint32_t)fmin(ms, UINT32_MAX);
    if (isfinite(mg) && mg > 0) job->target_grams = mg / 1000.0;
    return true;
}

bool gbs_latest_coffee(const cJSON *json, gbs_coffee_t *coffee)
{
    memset(coffee, 0, sizeof(*coffee));
    const cJSON *output = cJSON_GetObjectItemCaseSensitive(json, "output");
    const cJSON *list = cJSON_GetObjectItemCaseSensitive(output, "lastCoffees");
    if (!cJSON_IsArray(list)) return false;
    const cJSON *item;
    cJSON_ArrayForEach(item, list) {
        double when = number(item, "time");
        if (!isfinite(when) || when <= coffee->time_ms || when >= 9007199254740991.0) continue;
        coffee->time_ms = (int64_t)when;
        coffee->usable = false;
        coffee->duration_ms = 0;
        coffee->mass_mg = 0;
        const cJSON *valid = cJSON_GetObjectItemCaseSensitive(item, "valid");
        if (valid && !cJSON_IsTrue(valid)) continue;
        double seconds = number(item, "extractionSeconds"), grams = number(item, "doseValue");
        // Use measured weight only: missing scale data must never become an invented result.
        if (!isfinite(seconds) || seconds <= 0 || seconds * 1000 > UINT32_MAX ||
            !isfinite(grams) || grams <= 0 || grams > 5000) continue;
        coffee->duration_ms = (uint32_t)round(seconds * 1000);
        coffee->mass_mg = (uint32_t)round(round(grams * 10) * 100);
        coffee->usable = coffee->duration_ms > 0 && coffee->mass_mg > 0;
    }
    return true;
}

bool gbs_seen(const gbs_ledger_t *ledger, const char *uuid)
{
    for (unsigned i = 0; i < GBS_SEEN_CAP; ++i)
        if (strcmp(ledger->seen[i], uuid) == 0) return true;
    return false;
}

bool gbs_ledger_valid(const gbs_ledger_t *ledger)
{
    if (ledger->version != 1 || ledger->cursor >= GBS_SEEN_CAP ||
        !memchr(ledger->inflight, 0, sizeof(ledger->inflight)) ||
        !memchr(ledger->last_outcome, 0, sizeof(ledger->last_outcome))) return false;
    if (ledger->inflight[0] && !gbs_uuid_valid(ledger->inflight)) return false;
    for (unsigned i = 0; i < GBS_SEEN_CAP; ++i)
        if (!memchr(ledger->seen[i], 0, sizeof(ledger->seen[i])) ||
            (ledger->seen[i][0] && !gbs_uuid_valid(ledger->seen[i]))) return false;
    return ledger->watermark_ms >= 0;
}

void gbs_remember(gbs_ledger_t *ledger, const char *uuid)
{
    if (!gbs_uuid_valid(uuid) || gbs_seen(ledger, uuid)) return;
    memcpy(ledger->seen[ledger->cursor % GBS_SEEN_CAP], uuid, 37);
    ledger->cursor = (ledger->cursor + 1) % GBS_SEEN_CAP;
}

bool gbs_new_coffee(const gbs_coffee_t *coffee, int64_t watermark_ms)
{
    return coffee->time_ms > watermark_ms && coffee->usable;
}

static cJSON *paired(uint32_t pairing, const gbs_job_t *job)
{
    cJSON *body = cJSON_CreateObject();
    if (!body) return NULL;
    if (!cJSON_AddNumberToObject(body, "version", 1) ||
        !cJSON_AddNumberToObject(body, "pairingId", pairing) ||
        !cJSON_AddStringToObject(body, "grindUuid", job->uuid)) {
        cJSON_Delete(body); return NULL;
    }
    return body;
}

cJSON *gbs_accept_body(uint32_t pairing, const gbs_job_t *job)
{
    cJSON *body = paired(pairing, job), *dose = cJSON_Parse(job->dose_json);
    if (!body || !dose || !cJSON_AddNumberToObject(body, "groupHeadId", job->group)) {
        cJSON_Delete(body); cJSON_Delete(dose); return NULL;
    }
    if (!cJSON_AddItemToObject(body, "doseType", dose)) {
        cJSON_Delete(dose); cJSON_Delete(body); return NULL;
    }
    return body;
}

cJSON *gbs_done_body(uint32_t pairing, const gbs_job_t *job,
                     const gbs_coffee_t *coffee, const char *event_uuid, bool canceled)
{
    if (!gbs_uuid_valid(event_uuid) || (!canceled && !coffee->usable)) return NULL;
    cJSON *body = paired(pairing, job);
    if (!body) return NULL;
    if (!cJSON_AddNumberToObject(body, "durationFinalMs", canceled ? 0 : coffee->duration_ms) ||
        !cJSON_AddNumberToObject(body, "massFinalMg", canceled ? 0 : coffee->mass_mg) ||
        !cJSON_AddStringToObject(body, "brewEventUuid", event_uuid) ||
        !cJSON_AddStringToObject(body, "result", canceled ? "Canceled" : "Success")) {
        cJSON_Delete(body); return NULL;
    }
    return body;
}

void gbs_proof_fold(const char *input, const unsigned char secret[32], unsigned char work[32])
{
    memcpy(work, secret, 32);
    for (const unsigned char *p = (const unsigned char *)input; *p; ++p) {
        unsigned index = *p % 32, shift = work[(index + 1) % 32] & 7;
        unsigned x = *p ^ work[index];
        work[index] = (unsigned char)((x << shift) | (x >> (8 - shift)));
    }
}
