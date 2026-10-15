#pragma once
#include "esp_err.h"
#include "logic.h"

typedef struct {
    char wifi_ssid[33], wifi_password[64];
    char username[256], password[256], serial[64];
    char grinder_host[16], device_id[37];
    char grinder_cert[4096];
    uint32_t pairing_id;
    char installation_id[37], public_key[160], private_key[256], registration_proof[64];
    unsigned char secret[32];
} gbs_config_t;

extern gbs_config_t gbs_config;
esp_err_t gbs_load_config(void);
esp_err_t gbs_crypto_init(void);
void gbs_uuid(char output[37]);
int64_t gbs_now_ms(void);
esp_err_t gbs_cloud_connect(void);
esp_err_t gbs_cloud_coffee(gbs_coffee_t *coffee);
// Response ownership passes to caller. API status is checked separately.
esp_err_t gbs_grinder(const char *endpoint, const cJSON *body, cJSON **response,
                      char status[80]);
