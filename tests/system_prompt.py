"""Called by e2e.sh against a running launcher + fake upstream."""
import json, sys, urllib.request
api, router, pid = sys.argv[1:]
def call(url, body=None, method=None, raw=False):
    data = None if body is None else json.dumps(body, ensure_ascii=False).encode()
    with urllib.request.urlopen(urllib.request.Request(url, data=data, method=method, headers={'content-type': 'application/json'}), timeout=30) as r:
        result = r.read().decode()
        return result if raw else json.loads(result)

preset_url = api + '/presets/' + pid
original = call(preset_url)
prompt = 'Ты помощник.\nОтвечай кратко.\n'
user = {'role': 'user', 'content': '__echo_messages__'}
def save(text=prompt, append=False):
    p = dict(original, system_prompt=text, system_prompt_append=append)
    return call(preset_url, p, 'PUT')
def echo(messages, endpoint=None, anthropic=False, **extra):
    body = dict(model='coder', messages=messages, **extra)
    if anthropic: body['max_tokens'] = 100
    resp = call(endpoint or router + '/v1/chat/completions', body)
    return json.loads(resp['content'][0]['text'] if anthropic else resp['choices'][0]['message']['content'])

def check(name, condition):
    assert condition, name
    print('  ok   ' + name)

try:
    check('old preset defaults', original['system_prompt'] == '' and original['system_prompt_append'] is False)
    saved = save()
    check('preset prompt saved', saved['system_prompt'] == prompt and saved['system_prompt_append'] is False)
    for endpoint in [router + '/v1/chat/completions?test=1', api + '/v1/chat/completions', router + '/instance/coder/v1/chat/completions', router + '/instance/coder/chat/completions']:
        check('preset prompt routed ' + endpoint.split(':')[-1], echo([user], endpoint) == [{'role': 'system', 'content': prompt}, user])
    for role in ['system', 'developer']:
        for content in ['Client instructions', '', [{'type': 'text', 'text': 'Client instructions'}]]:
            messages = [{'role': role, 'content': content}, user]
            check('client ' + role + ' overrides preset', echo(messages) == messages)
    check('Anthropic preset fallback', echo([user], router + '/v1/messages', anthropic=True)[0]['content'] == prompt)
    check('Anthropic explicit prompt wins', echo([user], router + '/v1/messages', anthropic=True, system='Client')[0]['content'] == 'Client')
    check('Anthropic empty explicit prompt wins', echo([user], router + '/v1/messages', anthropic=True, system='')[0]['content'] == '')
    save(append=True)
    for role in ['system', 'developer']:
        messages = [{'role': role, 'content': 'Client'}, user]
        check('append merges ' + role, echo(messages) == [{'role': role, 'content': prompt + '\n\nClient'}, user])
    parts = [{'type': 'text', 'text': 'Client', 'cache_control': {'type': 'ephemeral'}}]
    check('append preserves content parts', echo([{'role': 'system', 'content': parts}, user])[0]['content'] == [{'type': 'text', 'text': prompt + '\n\n'}] + parts)
    check('Anthropic append blocks', echo([user], router + '/v1/messages', anthropic=True, system=[{'type': 'text', 'text': 'Client'}])[0]['content'] == prompt + '\n\nClient')
    stream = call(router + '/v1/chat/completions', {'model': 'coder', 'messages': [user], 'stream': True}, raw=True)
    chunks = [json.loads(line[6:]) for line in stream.splitlines() if line.startswith('data: ') and line != 'data: [DONE]']
    check('streaming receives preset prompt', json.loads(chunks[0]['choices'][0]['delta']['content']) == [{'role': 'system', 'content': prompt}, user])
    for _ in range(3): check('preset prompt does not accumulate', echo([user]) == [{'role': 'system', 'content': prompt}, user])
    clone = call(preset_url + '/clone', {})
    try:
        check('clone preserves prompt and mode', clone['system_prompt'] == prompt and clone['system_prompt_append'] is True)
    finally: call(api + '/presets/' + clone['id'], method='DELETE')
    export = call(api + '/presets/export')
    exported = next(p for p in export['presets'] if p['id'] == pid)
    check('export preserves prompt and mode', exported['system_prompt'] == prompt and exported['system_prompt_append'] is True)
    before = {p['id'] for p in call(api + '/presets')}
    imported = call(api + '/presets/import', dict(export, presets=[exported], aliases=[], groups=[]))
    added = [p for p in call(api + '/presets') if p['id'] not in before]
    try:
        check('import preserves prompt and mode', imported['imported'] == 1 and len(added) == 1 and added[0]['system_prompt'] == prompt and added[0]['system_prompt_append'] is True)
    finally:
        for p in added: call(api + '/presets/' + p['id'], method='DELETE')
    save(append=True)  # history stores the previous version, not the current one
    history = call(preset_url + '/history')
    check('history preserves prompt and mode', any(p['system_prompt'] == prompt and p['system_prompt_append'] is True for p in history))
    save('preset instructions\n', False)
    count_body = {'model': 'coder', 'messages': [{'role': 'user', 'content': 'one two three'}]}
    count_url = router + '/v1/messages/count_tokens'
    check('count tokens includes default prompt', call(count_url, count_body)['input_tokens'] == 5)
    check('count tokens client override', call(count_url, dict(count_body, system='client\n'))['input_tokens'] == 4)
    save('preset instructions\n', True)
    check('count tokens combined prompts', call(count_url, dict(count_body, system='client\n'))['input_tokens'] == 6)
    save('Changed immediately', False)
    check('prompt changes without restart', echo([user])[0]['content'] == 'Changed immediately')
    save(' \n\t', True)
    check('blank prompt is no-op', echo([user]) == [user])
finally:
    call(preset_url, original, 'PUT')
