#!/bin/bash
# Start launcherd with demo data for manual/visual UI testing. Leaves it running.
ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=${T:-/tmp/lll-demo}; rm -rf $T; mkdir -p $T/models; export LLL_DATA_DIR=$T/data
python3 $ROOT/tests/make_gguf.py $T/models/tiny-model-Q4_K_M.gguf >/dev/null
python3 $ROOT/tests/make_gguf.py $T/models/Qwen3-30B-A3B-Q8_0.gguf >/dev/null
nohup $ROOT/target/debug/launcherd > $T/out.log 2>&1 &
for i in $(seq 1 50); do curl -s localhost:8090/api/auth/status >/dev/null && break; sleep 0.2; done
API=http://127.0.0.1:8090/api
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
BID=$(curl -s -XPOST $API/backends -H 'content-type: application/json' -d "{\"name\":\"llama.cpp CUDA\",\"dir\":\"$ROOT/tests/fake\"}" | j 'd["id"]')
curl -s -XPOST $API/model-dirs -H 'content-type: application/json' -d "{\"path\":\"$T/models\",\"recursive\":true}" >/dev/null
curl -s -XPOST $API/models/scan >/dev/null
MID=$(curl -s $API/models | j '[m for m in d if "tiny" in m["file_name"]][0]["id"]')
P=$(curl -s -XPOST $API/presets -H 'content-type: application/json' -d "{\"name\":\"Coder\",\"icon\":\"🧠\",\"model_id\":\"$MID\",\"backend_id\":\"$BID\",\"args\":{\"--ctx-size\":16384,\"--jinja\":true,\"--cache-type-k\":\"q8_0\"},\"group\":\"Работа\"}" | j 'd["id"]')
curl -s -XPOST $API/presets -H 'content-type: application/json' -d "{\"name\":\"Chat fast\",\"model_id\":\"$MID\",\"backend_id\":\"$BID\",\"args\":{\"--ctx-size\":4096},\"group\":\"Работа\"}" >/dev/null
curl -s -XPOST $API/aliases -H 'content-type: application/json' -d "{\"name\":\"coder\",\"target_type\":\"preset\",\"target_id\":\"$P\",\"is_primary\":true}" >/dev/null
curl -s -XPOST $API/presets/$P/start >/dev/null
sleep 3
curl -s -XPOST localhost:8080/v1/chat/completions -d '{"model":"coder","stream":true,"messages":[]}' >/dev/null
echo "demo running: http://127.0.0.1:8090  (data $T)"
