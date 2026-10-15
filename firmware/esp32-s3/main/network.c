// La Marzocco authentication follows gbs-anywhere PR16.
#include "network.h"
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include "esp_crt_bundle.h"
#include "esp_http_client.h"
#include "esp_log.h"
#include "esp_random.h"
#include "esp_timer.h"
#include "lwip/inet.h"
#include "mbedtls/base64.h"
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/md.h"
#include "mbedtls/pk.h"
#include "nvs.h"

#define CLOUD "https://lion.lamarzocco.io/api/customer-app/"
#define MAX_RESPONSE (128 * 1024)
static const char *TAG = "gbs_net";
gbs_config_t gbs_config;
static mbedtls_pk_context installation_key;
static mbedtls_entropy_context entropy;
static mbedtls_ctr_drbg_context random_context;
static char access_token[4096], refresh_token[4096];
static int64_t token_expires_us;
static bool registered;

static bool string_field(const cJSON *j, const char *key, char *out, size_t capacity, bool empty)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(j, key);
    if (!cJSON_IsString(v)) return false;
    size_t len = strlen(v->valuestring);
    if ((!empty && !len) || len >= capacity) return false;
    memcpy(out, v->valuestring, len + 1);
    return true;
}

esp_err_t gbs_load_config(void)
{
    nvs_handle_t nvs;
    esp_err_t err = nvs_open("gbs_cfg", NVS_READONLY, &nvs);
    if (err != ESP_OK) return err;
    size_t length = 0;
    err = nvs_get_str(nvs, "config", NULL, &length);
    if (err != ESP_OK || length < 2 || length > 8192) {
        nvs_close(nvs); return ESP_ERR_INVALID_SIZE;
    }
    char *text = calloc(1, length);
    if (!text) { nvs_close(nvs); return ESP_ERR_NO_MEM; }
    err = nvs_get_str(nvs, "config", text, &length);
    nvs_close(nvs);
    cJSON *j = err == ESP_OK ? cJSON_Parse(text) : NULL;
    memset(text, 0, length); free(text);
    if (!j) return ESP_ERR_INVALID_RESPONSE;
    gbs_config_t *c = &gbs_config;
    char secret[64];
    bool ok = string_field(j, "wifi_ssid", c->wifi_ssid, sizeof(c->wifi_ssid), false) &&
        string_field(j, "wifi_password", c->wifi_password, sizeof(c->wifi_password), false) &&
        string_field(j, "cloud_username", c->username, sizeof(c->username), false) &&
        string_field(j, "cloud_password", c->password, sizeof(c->password), false) &&
        string_field(j, "serial", c->serial, sizeof(c->serial), true) &&
        string_field(j, "grinder_host", c->grinder_host, sizeof(c->grinder_host), false) &&
        string_field(j, "grinder_cert", c->grinder_cert, sizeof(c->grinder_cert), false) &&
        string_field(j, "brew_device_id", c->device_id, sizeof(c->device_id), false) &&
        string_field(j, "installation_id", c->installation_id, sizeof(c->installation_id), false) &&
        string_field(j, "public_key", c->public_key, sizeof(c->public_key), false) &&
        string_field(j, "private_key", c->private_key, sizeof(c->private_key), false) &&
        string_field(j, "registration_proof", c->registration_proof, sizeof(c->registration_proof), false) &&
        string_field(j, "secret", secret, sizeof(secret), false);
    const cJSON *id = cJSON_GetObjectItemCaseSensitive(j, "pairing_id");
    const cJSON *schema = cJSON_GetObjectItemCaseSensitive(j, "schema");
    ok = ok && cJSON_IsNumber(id) && id->valuedouble > 0 && id->valuedouble <= 65535 &&
        floor(id->valuedouble) == id->valuedouble && cJSON_IsNumber(schema) && schema->valuedouble == 1;
    if (ok) c->pairing_id = (uint32_t)id->valuedouble;
    size_t decoded = 0;
    if (ok) ok = mbedtls_base64_decode(c->secret, sizeof(c->secret), &decoded,
                                      (unsigned char *)secret, strlen(secret)) == 0 && decoded == 32;
    cJSON_Delete(j);
    struct in_addr address;
    return ok && inet_pton(AF_INET, c->grinder_host, &address) == 1 &&
        strlen(c->wifi_password) >= 8 && gbs_uuid_valid(c->device_id) &&
        gbs_uuid_valid(c->installation_id) ? ESP_OK : ESP_ERR_INVALID_ARG;
}

int64_t gbs_now_ms(void)
{
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (int64_t)tv.tv_sec * 1000 + tv.tv_usec / 1000;
}

void gbs_uuid(char out[37])
{
    unsigned char b[16];
    esp_fill_random(b, sizeof(b));
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    snprintf(out, 37, "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
             b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
}

static bool base64(const unsigned char *input, size_t length, char *out, size_t size)
{
    size_t encoded = 0;
    if (mbedtls_base64_encode((unsigned char *)out, size - 1, &encoded, input, length) != 0) return false;
    out[encoded] = 0;
    return true;
}

static bool request_proof(const char *input, const unsigned char secret[32], char output[64])
{
    unsigned char work[32], hash[32];
    gbs_proof_fold(input, secret, work);
    if (mbedtls_md(mbedtls_md_info_from_type(MBEDTLS_MD_SHA256), work, sizeof(work), hash) != 0) return false;
    return base64(hash, sizeof(hash), output, 64);
}

esp_err_t gbs_crypto_init(void)
{
    char proof[64];
    unsigned char zero[32] = {0};
    if (!request_proof("abc", zero, proof) || strcmp(proof, "P5ynD3LZU7oRYIN0D2ZtOlbpepJdBXQH9vdqJGOfq/o="))
        return ESP_ERR_INVALID_RESPONSE;
    mbedtls_pk_init(&installation_key);
    mbedtls_entropy_init(&entropy);
    mbedtls_ctr_drbg_init(&random_context);
    const unsigned char personalization[] = "gbs-esp32";
    if (mbedtls_ctr_drbg_seed(&random_context, mbedtls_entropy_func, &entropy,
                             personalization, sizeof(personalization) - 1)) return ESP_FAIL;
    unsigned char der[256];
    size_t der_len = 0;
    if (mbedtls_base64_decode(der, sizeof(der), &der_len, (unsigned char *)gbs_config.private_key,
                              strlen(gbs_config.private_key))) return ESP_FAIL;
    int result = mbedtls_pk_parse_key(&installation_key, der, der_len, NULL, 0,
                                     mbedtls_ctr_drbg_random, &random_context);
    memset(der, 0, sizeof(der));
    memset(gbs_config.private_key, 0, sizeof(gbs_config.private_key));
    return result == 0 ? ESP_OK : ESP_FAIL;
}

static esp_err_t sign_headers(esp_http_client_handle_t client)
{
    char nonce[37], timestamp[24], material[256], proof[64], signature[160];
    gbs_uuid(nonce);
    snprintf(timestamp, sizeof(timestamp), "%" PRId64, gbs_now_ms());
    snprintf(material, sizeof(material), "%s.%s.%s", gbs_config.installation_id, nonce, timestamp);
    if (!request_proof(material, gbs_config.secret, proof)) return ESP_FAIL;
    size_t at = strlen(material);
    snprintf(material + at, sizeof(material) - at, ".%s", proof);
    unsigned char hash[32], der[96];
    size_t der_len;
    if (mbedtls_md(mbedtls_md_info_from_type(MBEDTLS_MD_SHA256),
                   (unsigned char *)material, strlen(material), hash) ||
        mbedtls_pk_sign(&installation_key, MBEDTLS_MD_SHA256, hash, sizeof(hash), der, sizeof(der),
                        &der_len, mbedtls_ctr_drbg_random, &random_context) ||
        !base64(der, der_len, signature, sizeof(signature))) return ESP_FAIL;
    if (esp_http_client_set_header(client, "X-App-Installation-Id", gbs_config.installation_id) != ESP_OK ||
        esp_http_client_set_header(client, "X-Timestamp", timestamp) != ESP_OK ||
        esp_http_client_set_header(client, "X-Nonce", nonce) != ESP_OK ||
        esp_http_client_set_header(client, "X-Request-Signature", signature) != ESP_OK) return ESP_ERR_NO_MEM;
    return ESP_OK;
}

typedef struct { char *data; size_t used, capacity; bool failed; } response_t;

static esp_err_t receive(esp_http_client_event_t *event)
{
    if (event->event_id != HTTP_EVENT_ON_DATA || event->data_len <= 0) return ESP_OK;
    response_t *r = event->user_data;
    size_t next = r->used + (size_t)event->data_len + 1;
    if (next > MAX_RESPONSE || r->failed) { r->failed = true; return ESP_ERR_INVALID_SIZE; }
    if (next > r->capacity) {
        size_t capacity = r->capacity ? r->capacity * 2 : 4096;
        if (capacity < next) capacity = next;
        if (capacity > MAX_RESPONSE) capacity = MAX_RESPONSE;
        char *buffer = realloc(r->data, capacity);
        if (!buffer) { r->failed = true; return ESP_ERR_NO_MEM; }
        r->data = buffer; r->capacity = capacity;
    }
    memcpy(r->data + r->used, event->data, event->data_len);
    r->used += event->data_len;
    r->data[r->used] = 0;
    return ESP_OK;
}

static esp_err_t request(const char *url, bool grinder, bool registration, const char *bearer,
                         const cJSON *body, int *http_status, cJSON **json)
{
    *http_status = 0; *json = NULL;
    response_t response = {0};
    esp_http_client_config_t config = {
        .url = url, .method = body ? HTTP_METHOD_POST : HTTP_METHOD_GET,
        .timeout_ms = grinder ? 5000 : 12000, .buffer_size = 4096, .buffer_size_tx = 4096,
        .event_handler = receive, .user_data = &response, .disable_auto_redirect = true,
        .cert_pem = grinder ? gbs_config.grinder_cert : NULL,
        .crt_bundle_attach = grinder ? NULL : esp_crt_bundle_attach,
        // This grinder's pinned self-signed certificate has no DNS/IP name.
        .skip_cert_common_name_check = grinder,
    };
    char *encoded = body ? cJSON_PrintUnformatted(body) : NULL;
    if (body && !encoded) return ESP_ERR_NO_MEM;
    esp_http_client_handle_t client = esp_http_client_init(&config);
    if (!client) { cJSON_free(encoded); return ESP_ERR_NO_MEM; }
    esp_err_t err = esp_http_client_set_header(client, "Content-Type", "application/json");
    if (err == ESP_OK) err = esp_http_client_set_header(client, "Accept-Encoding", "identity");
    if (err == ESP_OK) err = esp_http_client_set_header(client, "Connection", "close");
    if (err == ESP_OK && !grinder) {
        if (registration) {
            err = esp_http_client_set_header(client, "X-App-Installation-Id", gbs_config.installation_id);
            if (err == ESP_OK) err = esp_http_client_set_header(client, "X-Request-Proof", gbs_config.registration_proof);
        } else err = sign_headers(client);
    }
    char *authorization = NULL;
    if (err == ESP_OK && bearer) {
        if (asprintf(&authorization, "Bearer %s", bearer) < 0) err = ESP_ERR_NO_MEM;
        else err = esp_http_client_set_header(client, "Authorization", authorization);
    }
    if (err == ESP_OK && encoded) err = esp_http_client_set_post_field(client, encoded, strlen(encoded));
    if (err == ESP_OK) err = esp_http_client_perform(client);
    *http_status = esp_http_client_get_status_code(client);
    if (err == ESP_OK && (response.failed || !esp_http_client_is_complete_data_received(client)))
        err = ESP_ERR_INVALID_RESPONSE;
    if (err == ESP_OK && response.used) {
        *json = cJSON_ParseWithLength(response.data, response.used);
        if (!*json) err = ESP_ERR_INVALID_RESPONSE;
    }
    // Never log cloud response bodies, which include credentials/tokens during sign-in.
    esp_http_client_cleanup(client);
    if (authorization) { memset(authorization, 0, strlen(authorization)); free(authorization); }
    if (encoded) { memset(encoded, 0, strlen(encoded)); cJSON_free(encoded); }
    if (response.data) { memset(response.data, 0, response.used); free(response.data); }
    return err;
}

static esp_err_t cloud_request(const char *path, bool registration, const char *bearer,
                               const cJSON *body, int *status, cJSON **reply)
{
    *status = 0; *reply = NULL;
    char url[256];
    if (snprintf(url, sizeof(url), CLOUD "%s", path) >= sizeof(url)) return ESP_ERR_INVALID_SIZE;
    esp_err_t err = request(url, false, registration, bearer, body, status, reply);
    if (err != ESP_OK || *status < 200 || *status >= 300)
        ESP_LOGW(TAG, "cloud %s HTTP=%d transport=%s", path, *status, esp_err_to_name(err));
    return err;
}

static esp_err_t get_token(bool refresh)
{
    cJSON *body = cJSON_CreateObject(), *reply = NULL;
    if (!body) return ESP_ERR_NO_MEM;
    if (!cJSON_AddStringToObject(body, "username", gbs_config.username) ||
        !cJSON_AddStringToObject(body, refresh ? "refreshToken" : "password", refresh ? refresh_token : gbs_config.password)) {
        cJSON_Delete(body); return ESP_ERR_NO_MEM;
    }
    int status;
    esp_err_t err = cloud_request(refresh ? "auth/refreshtoken" : "auth/signin", false, NULL, body, &status, &reply);
    if (err == ESP_OK && (status < 200 || status >= 300 ||
        !string_field(reply, "accessToken", access_token, sizeof(access_token), false) ||
        !string_field(reply, "refreshToken", refresh_token, sizeof(refresh_token), false))) err = ESP_ERR_INVALID_RESPONSE;
    if (err == ESP_OK) token_expires_us = esp_timer_get_time() + 3600000000LL;
    cJSON_Delete(body); cJSON_Delete(reply);
    return err;
}

static esp_err_t cloud_get(const char *path, cJSON **reply)
{
    *reply = NULL;
    for (int attempt = 0; attempt < 2; ++attempt) {
        int64_t remaining = token_expires_us - esp_timer_get_time();
        if (!access_token[0] || remaining < 600000000LL) {
            esp_err_t err = ESP_FAIL;
            if (access_token[0] && remaining > 0) err = get_token(true);
            if (err != ESP_OK) err = get_token(false);
            if (err != ESP_OK) return err;
        }
        int status;
        esp_err_t err = cloud_request(path, false, access_token, NULL, &status, reply);
        if (err == ESP_OK && status >= 200 && status < 300 && *reply) return ESP_OK;
        cJSON_Delete(*reply); *reply = NULL;
        if (status == 401 && attempt == 0) { access_token[0] = 0; continue; }
        return err != ESP_OK ? err : ESP_ERR_INVALID_RESPONSE;
    }
    return ESP_FAIL;
}

esp_err_t gbs_cloud_connect(void)
{
    cJSON *reply = NULL;
    if (!registered) {
        cJSON *body = cJSON_CreateObject();
        if (!body || !cJSON_AddStringToObject(body, "pk", gbs_config.public_key)) {
            cJSON_Delete(body); return ESP_ERR_NO_MEM;
        }
        int status;
        esp_err_t err = cloud_request("auth/init", true, NULL, body, &status, &reply);
        cJSON_Delete(body); cJSON_Delete(reply); reply = NULL;
        if (err != ESP_OK || status < 200 || status >= 300) return ESP_FAIL;
        registered = true;
    }
    esp_err_t err = cloud_get("things", &reply);
    if (err != ESP_OK) return err;
    const cJSON *thing, *chosen = NULL;
    int matches = 0;
    if (cJSON_IsArray(reply)) cJSON_ArrayForEach(thing, reply) {
        const cJSON *type = cJSON_GetObjectItemCaseSensitive(thing, "type");
        if (cJSON_IsString(type) && type->valuestring[0] && strcmp(type->valuestring, "CoffeeMachine")) continue;
        const cJSON *serial = cJSON_GetObjectItemCaseSensitive(thing, "serialNumber");
        if (!cJSON_IsString(serial)) continue;
        if (gbs_config.serial[0] && strcasecmp(gbs_config.serial, serial->valuestring)) continue;
        chosen = thing; ++matches;
    }
    bool ok = matches == 1 && string_field(chosen, "serialNumber", gbs_config.serial, sizeof(gbs_config.serial), false);
    if (ok) {
        for (const char *p = gbs_config.serial; *p; ++p)
            if (!((*p >= 'A' && *p <= 'Z') || (*p >= 'a' && *p <= 'z') || (*p >= '0' && *p <= '9') || *p == '-')) ok = false;
    }
    cJSON_Delete(reply);
    return ok ? ESP_OK : ESP_ERR_INVALID_RESPONSE;
}

esp_err_t gbs_cloud_coffee(gbs_coffee_t *coffee)
{
    if (!gbs_config.serial[0]) return ESP_ERR_INVALID_STATE;
    char path[160];
    snprintf(path, sizeof(path), "things/%s/stats/LAST_COFFEE/1?days=1", gbs_config.serial);
    cJSON *reply = NULL;
    esp_err_t err = cloud_get(path, &reply);
    if (err == ESP_OK && !gbs_latest_coffee(reply, coffee)) err = ESP_ERR_INVALID_RESPONSE;
    cJSON_Delete(reply);
    return err;
}

esp_err_t gbs_grinder(const char *endpoint, const cJSON *body, cJSON **reply, char status[80])
{
    char url[160];
    snprintf(url, sizeof(url), "https://%s:30001/grinderApi/%s", gbs_config.grinder_host, endpoint);
    int http_status = 0;
    strcpy(status, "TransportError");
    esp_err_t err = request(url, true, false, NULL, body, &http_status, reply);
    if (err == ESP_OK && !string_field(*reply, "status", status, 80, false)) err = ESP_ERR_INVALID_RESPONSE;
    ESP_LOGI(TAG, "grinder %s HTTP=%d status=%s transport=%s", endpoint, http_status, status, esp_err_to_name(err));
    if (err == ESP_OK && !gbs_reply_valid(http_status, status)) err = ESP_ERR_INVALID_RESPONSE;
    return err;
}
