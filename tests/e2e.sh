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
check "wrapped allowed values" "$(curl -s $API/backends/$BID/schema | j "[c for p in d[\"params\"] if p[\"id\"]==\"--cache-type-k\" for c in p[\"choices\"]]")" "['f16', 'q8_0', 'q4_0', 'turbo2', 'turbo3', 'turbo4']"
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
# Anthropic Messages API
AM=$(curl -s -XPOST $RT/v1/messages -H 'content-type: application/json' -H 'anthropic-version: 2023-06-01' -d '{"model":"coder","max_tokens":50,"messages":[{"role":"user","content":"hi"}]}')
check "anthropic message" "$(echo "$AM" | j 'd["type"], d["content"][0]["text"], d["usage"]["input_tokens"], d["stop_reason"]')" "message hi from coder 5 end_turn"
AS=$(curl -sN -XPOST $RT/v1/messages -H 'content-type: application/json' -d '{"model":"coder","max_tokens":50,"stream":true,"messages":[{"role":"user","content":"hi"}]}')
check "anthropic stream events" "$(echo "$AS" | grep '^event:' | sort | uniq -c | awk '{print $3":"$1}' | tr '\n' ' ')" "content_block_delta:6 content_block_start:2 content_block_stop:2 message_delta:1 message_start:1 message_stop:1 ping:1 "
check "anthropic thinking+text" "$(echo "$AS" | grep -c 'thinking_delta')/$(echo "$AS" | grep -o '"text": *"[^"]*"' | tr -d '\n' | grep -c coder)" "3/1"
check "anthropic stream usage" "$(echo "$AS" | grep -A1 '^event: message_delta' | tail -1 | sed 's/^data: //' | j 'd["usage"]["output_tokens"], d["usage"]["cache_read_input_tokens"]')" "3 20"
AT=$(curl -s -XPOST $RT/v1/messages -H 'content-type: application/json' -d '{"model":"coder","max_tokens":50,"tools":[{"name":"echo","description":"e","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"use tool"}]}')
check "anthropic tool_use" "$(echo "$AT" | j 'd["stop_reason"], d["content"][0]["type"], d["content"][0]["name"], d["content"][0]["input"]')" "tool_use tool_use echo {'text': 'ping'}"
AR=$(curl -s -XPOST $RT/v1/messages -H 'content-type: application/json' -d '{"model":"coder","max_tokens":50,"tools":[{"name":"echo","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"use tool"},{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"echo","input":{"text":"ping"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"pong"}]}]}')
check "anthropic tool_result" "$(echo "$AR" | j 'd["content"][0]["text"]')" "tool said: pong"
check "anthropic count_tokens" "$(curl -s -XPOST $RT/v1/messages/count_tokens -H 'content-type: application/json' -d '{"model":"coder","messages":[{"role":"user","content":"one two three"}]}' | j 'd["input_tokens"]')" "3"
check "anthropic error format" "$(curl -s -XPOST $RT/v1/messages -H 'content-type: application/json' -d '{"model":"nope","max_tokens":5,"messages":[]}' | j 'd["type"], d["error"]["type"]')" "error not_found_error"
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
# game mode: a process from the list unloads models and blocks autoload
curl -s -XPUT $API/settings -H 'content-type: application/json' -d '{"game_processes":["fakegame.exe"],"game_grace_secs":1}' >/dev/null
curl -s -XPOST $RT/v1/chat/completions -d '{"model":"coder"}' >/dev/null
curl -s -XPOST $API/instances/$PID1/pin -H 'content-type: application/json' -d '{"pinned":true}' >/dev/null
cp "$(command -v sleep)" $T/fakegame; $T/fakegame 60 & GPID=$!
sleep 7
check "game mode on" "$(curl -s $API/gamemode | j 'd["active"], d["reason"], d["source"], len(d["unloaded"])')" "True fakegame list 1"
check "game mode unloaded" "$(curl -s $API/instances | j '[i["state"] for i in d if i["preset_name"]=="Tiny"][0]')" "stopped"
GE=$(curl -s -XPOST $RT/v1/chat/completions -d '{"model":"coder"}')
check "game mode blocks autoload" "$(echo "$GE" | j '"игровой режим" in d["error"]["message"]')" "True"
kill $GPID; sleep 6
check "game mode off" "$(curl -s $API/gamemode | j 'd["active"]')" "False"
check "game manual on" "$(curl -s -XPOST $API/gamemode/activate | j 'd["active"], d["source"]')" "True manual"
check "game manual off" "$(curl -s -XPOST $API/gamemode/deactivate | j 'd["active"]')" "False"
curl -s -XPUT $API/settings -H 'content-type: application/json' -d '{"game_mode":false}' >/dev/null
# perplexity / KLD comparison of two quants (fake llama-perplexity)
python3 $ROOT/tests/make_gguf.py $T/models/tiny-model-Q8_0.gguf >/dev/null; curl -s -XPOST $API/models/scan >/dev/null
M8=$(curl -s $API/models | j '[m["id"] for m in d if "Q8_0" in m["file_name"]][0]')
printf 'some text for perplexity\n%.0s' $(seq 1 50) > $T/text.txt
PJ=$(curl -s -XPOST $API/perplexity -H 'content-type: application/json' -d "{\"model_ids\":[\"$MID\",\"$M8\"],\"base_model_id\":\"$M8\",\"backend_id\":\"$BID\",\"dataset\":\"$T/text.txt\",\"chunks\":10}" | j 'd["job_id"]')
for i in $(seq 1 30); do [ "$(curl -s $API/jobs | PJ=$PJ j '[x["status"] for x in d if x["id"]==__import__("os").environ["PJ"]][0]')" = "running" ] || break; sleep 0.3; done
PR=$(curl -s $API/perplexity)
check "ppl runs" "$(echo "$PR" | j 'len(d)')" "2"
check "ppl base" "$(echo "$PR" | j '[(r["quant"], r["is_base"], r["ppl"]) for r in d if r["is_base"]]')" "[('Q8_0', True, 6.0)]"
check "ppl kld" "$(echo "$PR" | j '[(r["quant"], r["ppl"], r["kld"], r["same_top"]) for r in d if not r["is_base"]]')" "[('Q4_K_M', 6.32, 0.042, 94.2)]"
# MCP: stdio server, list tools, call a tool
MS=$(curl -s -XPOST $API/mcp/servers -H 'content-type: application/json' -d "{\"name\":\"echo\",\"transport\":\"stdio\",\"command\":\"$ROOT/tests/fake/mcp-echo\",\"enabled\":true}")
MSID=$(echo "$MS" | j 'd["server"]["id"]')
check "mcp connected" "$(echo "$MS" | j 'd["status"]["connected"], d["status"]["tools"], d["status"]["server_name"]')" "True 2 echo-test"
check "mcp tools" "$(curl -s $API/mcp/tools | j '[t["function"] for t in d["tools"]]')" "['echo__echo', 'echo__add']"
check "mcp call" "$(curl -s -XPOST $API/mcp/call -H 'content-type: application/json' -d "{\"server_id\":\"$MSID\",\"name\":\"add\",\"arguments\":{\"a\":2,\"b\":3}}" | j 'd["text"]')" "5"
check "mcp bad server" "$(curl -s -XPOST $API/mcp/servers -H 'content-type: application/json' -d '{"name":"bad","command":"/nonexistent/mcp","enabled":true}' | j 'd["status"]["connected"]')" "False"
# PWA files + optional HTTPS listener
check "manifest" "$(curl -s -o /dev/null -w '%{content_type}' http://127.0.0.1:8090/manifest.webmanifest)" "application/manifest+json"
check "sw no-cache" "$(curl -sI http://127.0.0.1:8090/sw.js | grep -ci 'cache-control: no-cache')" "1"
openssl req -x509 -newkey rsa:2048 -nodes -keyout $T/k.pem -out $T/c.pem -days 1 -subj /CN=localhost >/dev/null 2>&1
curl -s -XPUT $API/settings -H 'content-type: application/json' -d "{\"https_enabled\":true,\"https_port\":18443,\"https_cert\":\"$T/c.pem\",\"https_key\":\"$T/k.pem\"}" >/dev/null
for i in $(seq 1 30); do curl -sk https://127.0.0.1:18443/api/version >/dev/null && break; sleep 0.2; done
check "https" "$(curl -sk https://127.0.0.1:18443/api/version | j 'd["version"] != ""')" "True"
check "https in status" "$(curl -s $API/status | j 'd["https_port"]')" "18443"
curl -s -XPUT $API/settings -H 'content-type: application/json' -d '{"https_enabled":false}' >/dev/null
sleep 1.5
curl -s -XPOST $API/instances/stop-all >/dev/null
[ $fail = 0 ] && echo "E2E: ALL PASSED" || { echo "E2E: FAILURES"; tail -20 $T/out.log; exit 1; }
