#!/bin/bash
# End-to-end test of launcherd with a fake llama-server.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d); export LLL_DATA_DIR=$T/data
mkdir -p $T/models && python3 $ROOT/tests/make_gguf.py $T/models/tiny-model-Q4_K_M.gguf
$ROOT/target/debug/launcherd > $T/out.log 2>&1 &
PID=$!; trap "kill $PID 2>/dev/null; sleep 0.5; pkill -f tests/fake/llama-server; rm -rf $T" EXIT
for i in $(seq 1 50); do curl -s localhost:8090/api/auth/status >/dev/null && break; sleep 0.2; done
API=http://127.0.0.1:8090/api; RT=http://127.0.0.1:8080
fail=0; check() { if [ "$2" = "$3" ]; then echo "  ok   $1"; else echo "  FAIL $1: got '$2' want '$3'"; fail=1; fi; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

B=$(curl -s -XPOST $API/backends -H 'content-type: application/json' -d "{\"name\":\"fake-cuda\",\"dir\":\"$ROOT/tests/fake\"}")
BID=$(echo "$B" | j 'd["id"]'); check "backend scanned" "$(echo "$B" | j 'd["version"]')" "6500"
check "backend devices" "$(echo "$B" | j 'd["devices"][0]["id"]')" "CUDA0"
check "schema has turbo3" "$(curl -s $API/backends/$BID/schema | j '"turbo3" in [c for p in d["params"] if p["id"]=="--cache-type-k" for c in p["choices"]]')" "True"
curl -s -XPOST $API/model-dirs -H 'content-type: application/json' -d "{\"path\":\"$T/models\",\"recursive\":true}" >/dev/null
check "scan added" "$(curl -s -XPOST $API/models/scan | j 'd["added"]')" "1"
MID=$(curl -s $API/models | j 'd[0]["id"]')
check "model alias" "$(curl -s $API/models | j 'd[0]["alias"]')" "tiny-model-q4_k_m"
P=$(curl -s -XPOST $API/presets -H 'content-type: application/json' -d "{\"name\":\"Tiny\",\"model_id\":\"$MID\",\"backend_id\":\"$BID\",\"args\":{\"--ctx-size\":2048,\"--jinja\":true,\"--mmap\":false},\"ttl\":3}")
PID1=$(echo "$P" | j 'd["id"]')
curl -s -XPOST $API/aliases -H 'content-type: application/json' -d "{\"name\":\"coder\",\"target_type\":\"preset\",\"target_id\":\"$PID1\",\"is_primary\":true}" >/dev/null
check "command preview" "$(curl -s $API/presets/$PID1/command | j '"--no-mmap" in d["cmd"] and "--alias coder" in d["cmd"]')" "True"
check "estimate" "$(curl -s -XPOST $API/memory/estimate -H 'content-type: application/json' -d "{\"preset\":$P}" | j 'd["estimate"]["n_layers"]')" "4"
check "/v1/models" "$(curl -s $RT/v1/models | j 'd["data"][0]["id"]')" "coder"
R=$(curl -s -XPOST $RT/v1/chat/completions -H 'content-type: application/json' -d '{"model":"coder","messages":[{"role":"user","content":"hi"}]}')
check "router autostart + proxy" "$(echo "$R" | j 'd["choices"][0]["message"]["content"]')" "hi from coder"
S=$(curl -sN -XPOST $RT/v1/chat/completions -H 'content-type: application/json' -d '{"model":"coder","stream":true,"messages":[]}')
check "streaming" "$(echo "$S" | grep -c 'data:')" "8"
check "reasoning streamed" "$(echo "$S" | grep -c 'reasoning_content')" "3"
# chunks must arrive incrementally (not buffered until the end)
check "realtime chunks" "$(python3 -c "
import urllib.request,time,json
r=urllib.request.urlopen(urllib.request.Request('$RT/v1/chat/completions',data=json.dumps({'model':'coder','stream':True,'messages':[]}).encode(),headers={'content-type':'application/json'}))
t=[]
for line in r:
    if line.startswith(b'data:'): t.append(time.time())
print(t[-1]-t[0] > 0.2)")" "True"
check "unknown model 404" "$(curl -s -o /dev/null -w '%{http_code}' -XPOST $RT/v1/chat/completions -d '{"model":"nope"}')" "404"
check "instance ready" "$(curl -s $API/instances | j 'd[0]["state"]')" "ready"
check "logs captured" "$(curl -s "$API/instances/$PID1/logs?tail=50" | j 'any("offloaded" in l["line"] for l in d)')" "True"
check "offload parsed" "$(curl -s $API/instances | j 'd[0]["offloaded"]')" "33/33"
sleep 1
check "request log tps" "$(curl -s $API/router/requests | j 'd[0].get("tps")')" "42.5"
check "chat proxy via UI api" "$(curl -s -XPOST $API/v1/chat/completions -H 'content-type: application/json' -d '{"model":"coder","messages":[]}' | j 'd["model"]')" "coder"
echo "  .. waiting for TTL unload"; sleep 10
check "ttl unloaded" "$(curl -s $API/instances | j 'd[0]["state"]')" "stopped"
# failing preset → OOM hint
P2=$(curl -s -XPOST $API/presets -H 'content-type: application/json' -d "{\"name\":\"Broken\",\"model_id\":\"$MID\",\"backend_id\":\"$BID\",\"extra_args\":\"--fail-load\"}" | j 'd["id"]')
curl -s -XPOST $API/aliases -H 'content-type: application/json' -d "{\"name\":\"broken\",\"target_type\":\"preset\",\"target_id\":\"$P2\",\"is_primary\":true}" >/dev/null
E=$(curl -s -XPOST $RT/v1/chat/completions -d '{"model":"broken"}')
check "oom hint" "$(echo "$E" | j '"Не хватило памяти GPU" in d["error"]["message"]')" "True"
# group alias falls back
G=$(curl -s -XPOST $API/alias-groups -H 'content-type: application/json' -d "{\"name\":\"chat\",\"preset_ids\":[\"$PID1\",\"$P2\"]}" | j 'd["id"]')
curl -s -XPOST $API/aliases -H 'content-type: application/json' -d "{\"name\":\"chat\",\"target_type\":\"group\",\"target_id\":\"$G\",\"is_primary\":false}" >/dev/null
GR=$(curl -s -XPOST $RT/v1/chat/completions -d "{\"model\":\"chat\"}"); check "group alias" "$(echo "$GR" | j 'd["choices"][0]["message"]["content"]')" "hi from coder"
# import command
check "import command" "$(curl -s -XPOST $API/presets/import-command -H 'content-type: application/json' -d "{\"backend_id\":\"$BID\",\"command\":\"llama-server -m $T/models/tiny-model-Q4_K_M.gguf -c 16384 -ctk q8_0 --no-mmap\"}" | MID=$MID j 'd["model_id"]==__import__("os").environ["MID"] and d["imported"]["args"]["--ctx-size"]==16384')" "True"
# remote (via proxy header) needs auth
check "remote needs auth" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 192.168.1.50' $API/presets)" "401"
curl -s -XPOST $API/auth/password -H 'content-type: application/json' -d '{"password":"supersecret1"}' >/dev/null
TOK=$(curl -s -XPOST $API/auth/login -H 'X-Forwarded-For: 192.168.1.50' -H 'content-type: application/json' -d '{"password":"supersecret1"}' | j 'd["access_token"]')
check "remote with jwt" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 192.168.1.50' -H "Authorization: Bearer $TOK" $API/presets)" "200"
check "bad password" "$(curl -s -o /dev/null -w '%{http_code}' -XPOST $API/auth/login -H 'X-Forwarded-For: 192.168.1.51' -H 'content-type: application/json' -d '{"password":"nope"}')" "401"
check "open router w/o keys" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 10.0.0.5' -XPOST $RT/v1/chat/completions -d '{"model":"coder"}')" "200"
KEY=$(curl -s -XPOST $API/keys -H 'content-type: application/json' -d '{"name":"webui","scopes":["coder"],"can_load":true}' | j 'd["secret"]')
check "key allowed alias" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 10.0.0.5' -H "Authorization: Bearer $KEY" -XPOST $RT/v1/chat/completions -d '{"model":"coder"}')" "200"
check "key forbidden alias" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 10.0.0.5' -H "Authorization: Bearer $KEY" -XPOST $RT/v1/chat/completions -d '{"model":"broken"}')" "403"
check "key cannot manage" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 10.0.0.5' -H "Authorization: Bearer $KEY" $API/presets)" "403"
check "no key remote router" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 10.0.0.5' -XPOST $RT/v1/chat/completions -d '{"model":"coder"}')" "401"
sleep 0.5
AN="$API/analytics?from=0&to=99999999999"
check "usage cached tokens" "$(curl -s "$AN&group=model" | j 'd["totals"]["cached"] >= 40 and d["totals"]["prompt"] > d["totals"]["cached"]')" "True"
check "usage by model" "$(curl -s "$AN&group=model" | j 'd["groups"][0]["key"]')" "tiny-model-Q4_K_M.gguf"
check "usage by preset" "$(curl -s "$AN&group=preset" | j 'any(g["label"]=="Tiny" for g in d["groups"])')" "True"
check "usage by user (key)" "$(curl -s "$AN&group=user" | j '[(g["label"],g["requests"]) for g in d["groups"] if g["label"]=="webui"]')" "[('webui', 1)]"
check "usage series" "$(curl -s "$AN&group=alias&bucket=86400" | j 'sum(x["completion"] for x in d["series"]) == d["totals"]["completion"] > 0')" "True"
check "usage csv" "$(curl -s "$API/analytics/export?from=0&to=99999999999" | head -1)" "ts,preset,model,alias,user,ip,path,status,prompt,cached,completion,ms,tps"
check "export" "$(curl -s $API/presets/export | j 'len(d["presets"])')" "2"
check "backup zip" "$(curl -s $API/backup | head -c 2)" "PK"
check "ui served" "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8090/)" "200"
check "version" "$(curl -s $API/version | j 'd["version"]')" "$(grep -m1 '^version' ../Cargo.toml | cut -d'"' -f2)"
curl -s -XPOST $API/instances/stop-all >/dev/null
[ $fail = 0 ] && echo "E2E: ALL PASSED" || { echo "E2E: FAILURES"; tail -20 $T/out.log; exit 1; }
