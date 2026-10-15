idf_component_register(SRCS "main.c" "network.c" "logic.c"
    INCLUDE_DIRS "."
    REQUIRES esp_wifi esp_event esp_netif nvs_flash esp_http_client esp_http_server
             esp_timer mbedtls json console esp_psram
    EMBED_TXTFILES "index.html")
