#include "logic.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

static const char *ID = "11111111-1111-4111-8111-111111111111";
static const char *EVENT = "22222222-2222-4222-8222-222222222222";

static cJSON *json(const char *text) { cJSON *j = cJSON_Parse(text); assert(j); return j; }

static void jobs(void)
{
    cJSON *j = json("{\"grindUuid\":\"11111111-1111-4111-8111-111111111111\",\"groupHeadId\":null,\"doseTypeExpected\":2,\"durationExpectedMs\":30000,\"brewWeightExpectedMg\":-2147483648}");
    gbs_job_t job;
    assert(gbs_parse_job(j, &job));
    assert(job.group == 0 && job.target_ms == 30000 && job.target_grams == 0);
    cJSON *accept = gbs_accept_body(12345, &job);
    assert(accept);
    assert(cJSON_GetObjectItem(accept, "groupHeadId")->valueint == 0);
    assert(cJSON_GetObjectItem(accept, "doseType")->valueint == 2);
    assert(cJSON_GetObjectItem(accept, "pairingId")->valueint == 12345);
    cJSON_Delete(accept);
    cJSON_ReplaceItemInObject(j, "groupHeadId", cJSON_CreateNumber(-1));
    assert(!gbs_parse_job(j, &job));
    cJSON_ReplaceItemInObject(j, "groupHeadId", cJSON_CreateNumber(0.5));
    assert(!gbs_parse_job(j, &job));
    cJSON_Delete(j);
    j = json("{\"grindUuid\":\"bad\"}");
    assert(!gbs_parse_job(j, &job)); cJSON_Delete(j);
    assert(!gbs_uuid_valid(NULL)); assert(!gbs_uuid_valid("../api"));
    assert(gbs_uuid_valid(ID));
}

static void coffee(void)
{
    cJSON *j = json("{\"output\":{\"lastCoffees\":[{\"time\":1791942600000,\"extractionSeconds\":37.110,\"doseValue\":29.2999992},{\"time\":1791942599000,\"extractionSeconds\":30,\"doseValue\":30}]}}");
    gbs_coffee_t c;
    assert(gbs_latest_coffee(j, &c));
    assert(c.usable && c.duration_ms == 37110 && c.mass_mg == 29300);
    assert(gbs_new_coffee(&c, c.time_ms - 1));
    assert(!gbs_new_coffee(&c, c.time_ms));
    assert(!gbs_new_coffee(&c, c.time_ms + 1));
    cJSON *list = cJSON_GetObjectItem(cJSON_GetObjectItem(j, "output"), "lastCoffees");
    cJSON *newer = json("{\"time\":1791942700000,\"extractionSeconds\":5,\"doseValue\":3,\"valid\":false}");
    cJSON_AddItemToArray(list, newer);
    assert(gbs_latest_coffee(j, &c));
    assert(!c.usable && c.time_ms == 1791942700000);
    assert(!gbs_new_coffee(&c, 0)); // Never fall back to an older valid cup.
    cJSON_Delete(j);
    j = json("{\"output\":{\"lastCoffees\":[{\"time\":1791942600000,\"extractionSeconds\":37.11}]}}");
    assert(gbs_latest_coffee(j, &c) && !c.usable); cJSON_Delete(j);
    j = json("{\"output\":{\"lastCoffees\":[]}}");
    assert(gbs_latest_coffee(j, &c) && c.time_ms == 0); cJSON_Delete(j);
    j = json("{\"output\":{}}");
    assert(!gbs_latest_coffee(j, &c)); cJSON_Delete(j);
    j = json("{\"output\":{\"lastCoffees\":[{\"time\":1791942600000,\"extractionSeconds\":1e300,\"doseValue\":30}]}}");
    assert(gbs_latest_coffee(j, &c) && !c.usable); cJSON_Delete(j);
}

static void results(void)
{
    gbs_job_t job = {.group = 0};
    strcpy(job.uuid, ID); strcpy(job.dose_json, "null");
    gbs_coffee_t c = {.usable = true, .duration_ms = 37110, .mass_mg = 30000};
    cJSON *j = gbs_done_body(12345, &job, &c, EVENT, false);
    assert(j);
    assert(cJSON_GetObjectItem(j, "durationFinalMs")->valueint == 37110);
    assert(cJSON_GetObjectItem(j, "massFinalMg")->valueint == 30000);
    assert(!strcmp(cJSON_GetObjectItem(j, "result")->valuestring, "Success"));
    cJSON_Delete(j);
    c.usable = false;
    assert(!gbs_done_body(12345, &job, &c, EVENT, false));
    j = gbs_done_body(12345, &job, &c, EVENT, true);
    assert(j && cJSON_GetObjectItem(j, "durationFinalMs")->valueint == 0);
    assert(cJSON_GetObjectItem(j, "massFinalMg")->valueint == 0);
    assert(!strcmp(cJSON_GetObjectItem(j, "result")->valuestring, "Canceled"));
    cJSON_Delete(j);
}

static void dedup(void)
{
    gbs_ledger_t ledger = {.version = 1};
    assert(!gbs_seen(&ledger, ID));
    gbs_remember(&ledger, ID);
    assert(gbs_seen(&ledger, ID) && ledger.cursor == 1);
    gbs_remember(&ledger, ID);
    assert(ledger.cursor == 1);
    gbs_ledger_t rebooted = ledger;
    assert(gbs_seen(&rebooted, ID));
    assert(gbs_ledger_valid(&rebooted));
    memset(rebooted.inflight, 'a', sizeof(rebooted.inflight));
    assert(!gbs_ledger_valid(&rebooted));
    rebooted = ledger;
    rebooted.cursor = GBS_SEEN_CAP;
    assert(!gbs_ledger_valid(&rebooted));
    for (int i = 0; i < 100; ++i) {
        char id[37]; snprintf(id, sizeof(id), "%08x-1111-4111-8111-111111111111", i);
        gbs_remember(&ledger, id);
        assert(gbs_seen(&ledger, id) && ledger.cursor < GBS_SEEN_CAP);
    }
}

static void proof(void)
{
    unsigned char secret[32] = {0}, folded[32];
    gbs_proof_fold("abc", secret, folded);
    assert(folded[1] == 'a' && folded[2] == 'b' && folded[3] == 'c');
    for (int i = 4; i < 32; ++i) assert(folded[i] == 0);
}

int main(void)
{
    assert(gbs_reply_valid(200, "Ok"));
    assert(gbs_reply_valid(400, "ClientErrorOngoingJobNotExisting"));
    assert(gbs_reply_valid(403, "ClientErrorBrewJobNotExisting"));
    assert(!gbs_reply_valid(403, "Ok"));
    assert(!gbs_reply_valid(503, "ServerError"));
    assert(!gbs_reply_valid(200, ""));
    assert(!gbs_reply_valid(200, NULL));
    jobs(); coffee(); results(); dedup(); proof();
    puts("PASS: job schema, measured coffee selection, stale/invalid rejection, feedback schema, persistent dedup, proof fold");
}
