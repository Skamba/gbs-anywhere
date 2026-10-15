#!/bin/sh
set -eu
gbs_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
gbs_idf=${IDF_PATH:?Source ESP-IDF export.sh first}
gbs_python=${GBS_PYTHON:-python}
mkdir -p "$gbs_root/build-host"
${CC:-cc} -std=c11 -Wno-deprecated-declarations -fsanitize=address,undefined -g \
  -I "$gbs_idf/components/json/cJSON" -c "$gbs_idf/components/json/cJSON/cJSON.c" \
  -o "$gbs_root/build-host/cJSON.o"
${CC:-cc} -std=c11 -Wall -Wextra -Werror -fsanitize=address,undefined -g \
  -I "$gbs_root/main" -I "$gbs_idf/components/json/cJSON" \
  "$gbs_root/main/logic.c" "$gbs_root/tests/test_logic.c" \
  "$gbs_root/build-host/cJSON.o" -lm -o "$gbs_root/build-host/test_logic"
"$gbs_root/build-host/test_logic"
"$gbs_python" "$gbs_root/tests/test_provision.py"
"$gbs_python" "$gbs_root/tests/test_serial_log.py"
"${GBS_NODE:-node}" "$gbs_root/tests/test_ui.cjs"
