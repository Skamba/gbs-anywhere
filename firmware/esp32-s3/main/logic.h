#pragma once
#include <stdbool.h>
#include <stdint.h>
#include "cJSON.h"

#define GBS_SEEN_CAP 32
typedef struct {
    char uuid[37];
    char dose_json[128];
    uint32_t group;
    uint32_t target_ms;
    double target_grams;
} gbs_job_t;

typedef struct {
    int64_t time_ms;
    uint32_t duration_ms;
    uint32_t mass_mg;
    bool usable;
} gbs_coffee_t;

typedef struct {
    uint32_t version;
    uint32_t cursor;
    char seen[GBS_SEEN_CAP][37];
    char inflight[37];
    int64_t watermark_ms;
    uint32_t successes;
    uint32_t last_duration_ms;
    uint32_t last_mass_mg;
    char last_outcome[40];
} gbs_ledger_t;

bool gbs_uuid_valid(const char *s);
bool gbs_reply_valid(int http_status, const char *status);
bool gbs_parse_job(const cJSON *json, gbs_job_t *job);
// Empty lists succeed with time_ms == 0; malformed envelopes fail closed.
bool gbs_latest_coffee(const cJSON *json, gbs_coffee_t *coffee);
bool gbs_seen(const gbs_ledger_t *ledger, const char *uuid);
bool gbs_ledger_valid(const gbs_ledger_t *ledger);
void gbs_remember(gbs_ledger_t *ledger, const char *uuid);
bool gbs_new_coffee(const gbs_coffee_t *coffee, int64_t watermark_ms);
cJSON *gbs_accept_body(uint32_t pairing, const gbs_job_t *job);
cJSON *gbs_done_body(uint32_t pairing, const gbs_job_t *job,
                     const gbs_coffee_t *coffee, const char *event_uuid, bool canceled);
void gbs_proof_fold(const char *input, const unsigned char secret[32], unsigned char work[32]);
