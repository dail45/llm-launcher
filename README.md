# Llama Launcher

Лаунчер для **llama.cpp** под Windows: пресеты запуска, калькулятор памяти (VRAM/RAM), несколько сборок и форков, единый OpenAI-совместимый роутер с автозагрузкой и выгрузкой моделей, доступ по локальной сети с паролем и API-ключами, загрузка моделей с Hugging Face и сборок с GitHub, бенчмарки, встроенный чат.

## Состав

| Часть | Путь | Что это |
|---|---|---|
| Ядро | `crates/core` (`lll-core`) | Вся логика: процессы llama-server, оценка памяти, роутер, REST API, SQLite, мониторинг GPU |
| Desktop | `app` | Приложение Tauri 2: окно + трей, ядро работает внутри процесса |
| Headless | `crates/launcherd` | То же ядро без окна — для службы / Task Scheduler / сервера |
| UI | `ui` | Svelte 5 + Vite; встраивается в бинарник и открывается в окне или браузере |
| Тесты | `tests` | e2e-тест с фейковым llama-server (`tests/e2e.sh`), демо-данные (`tests/ui_demo.sh`) |

Порты по умолчанию: **8090** — интерфейс и API управления, **8080** — роутер OpenAI API (`/v1/...`), **51000–51999** — внутренние порты llama-server.

## Сборка

### Через GitHub Actions (проще всего)
Запушьте репозиторий на GitHub — `.github/workflows/build.yml` соберёт на `windows-latest`:
- `Llama Launcher_*_x64-setup.exe` (NSIS) и `.msi` — установщики;
- `LlamaLauncher-portable.exe` — без установки (нужен WebView2, в Windows 10/11 он обычно есть);
- `launcherd.exe` — версия без окна.

Тег `v*` дополнительно публикует GitHub Release.

### Локально на Windows
Нужны: [Rust](https://rustup.rs) (MSVC), Node.js 20+, Visual Studio Build Tools (C++).
```powershell
cd ui;  npm ci;  npm run build;  cd ..
cargo install tauri-cli --version "^2" --locked
cd app; cargo tauri build          # установщики в target\release\bundle
cargo build --release -p launcherd # target\release\launcherd.exe
```
Разработка UI: `cd ui; npm run dev` (Vite на :5173 проксирует `/api` на запущенный лаунчер :8090).

## Первый запуск
1. **Сборки llama.cpp** → «Добавить папку» (каталог с `llama-server.exe`) или «Скачать с GitHub» (официальный llama.cpp и форки; для CUDA предлагается докачать cudart). Лаунчер читает `--version`, `--help` и `--list-devices`: форма параметров строится по реальному `--help` каждой сборки, поэтому флаги форков (например, `turbo3`) появляются сами.
2. **Модели** → добавьте папку с `.gguf` (split-файлы и mmproj распознаются автоматически) или скачайте с **Hugging Face** — там же видно, влезет ли квант в вашу VRAM.
3. **Пресеты** → «Новый пресет» или «Импорт команды» (вставьте существующую строку `llama-server ...`). Справа калькулятор памяти по каждому GPU и кнопки автоподбора `-ngl`, контекста, `--n-cpu-moe`, `--tensor-split`.
4. Укажите **алиас** — по нему клиенты обращаются к модели: `model: "coder"`.

## Роутер (OpenAI API)
`http://<host>:8080/v1` — `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/models` и т.д.
- Поле `model` = алиас пресета, имя пресета или алиас группы. Незапущенный пресет стартует автоматически; при нехватке памяти выгружаются давно неиспользуемые незакреплённые модели.
- **Стриминг** (`"stream": true`) передаётся без буферизации, включая `reasoning_content` — размышления модели видны в реальном времени (используйте `--reasoning-format deepseek` в пресете, либо модель отдаёт `<think>` в тексте — встроенный чат понимает оба варианта).
- Группа алиасов: один алиас → несколько пресетов по приоритету (например, «2×GPU → 1×GPU → CPU»).
- Нативный API инстанса: `http://<host>:8080/instance/<алиас>/...` (например, `/props`, `/slots`).
- Выгрузка по простою: TTL в настройках (по умолчанию 600 с) или в пресете; 📌 закреплённые не выгружаются.

## Удалённый доступ по локальной сети
1. **Настройки → Безопасность**: задайте пароль (или `launcherd --set-password <пароль>`).
2. **Настройки → Сеть**: адрес интерфейса и/или роутера `0.0.0.0`. Можно ограничить подсети (CIDR).
3. Вход в интерфейс — по паролю (JWT, обновляемая сессия, защита от перебора). Клиентам роутера — **API-ключи** (`Authorization: Bearer lll_...`) с ограничением алиасов и права загрузки моделей.

Локальные запросы (127.0.0.1 без `X-Forwarded-For`) по умолчанию не требуют пароля — отключается в настройках.

### HTTPS через обратный прокси
HTTPS не встроен. Оставьте лаунчер на `127.0.0.1` и поставьте перед ним прокси; добавьте адрес прокси (`127.0.0.1`) в **«Доверенные прокси»**, чтобы лаунчер видел реальный IP клиента из `X-Forwarded-For` (для журнала, лимита попыток входа и фильтра подсетей). Прокси обязательно должен передавать `X-Forwarded-For` — запрос с loopback без этого заголовка считается локальным.

**Caddy** (`Caddyfile`):
```
llm.example.lan {
    tls internal
    reverse_proxy /v1/* 127.0.0.1:8080 { flush_interval -1 }
    reverse_proxy /instance/* 127.0.0.1:8080 { flush_interval -1 }
    reverse_proxy 127.0.0.1:8090 { flush_interval -1 }
}
```
**nginx**:
```nginx
server {
    listen 443 ssl;
    server_name llm.example.lan;
    # ssl_certificate ...; ssl_certificate_key ...;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_buffering off;            # стриминг токенов и SSE
    proxy_read_timeout 3600s;
    client_max_body_size 100m;
    location ~ ^/(v1|instance)/ { proxy_pass http://127.0.0.1:8080; }
    location / { proxy_pass http://127.0.0.1:8090; }
}
```

## Запуск как служба (без окна)
`launcherd.exe` — то же ядро без интерфейса окна (UI доступен в браузере).
- **Task Scheduler**: «Создать задачу» → триггер «При входе в систему» или «При запуске» → действие `C:\LlamaLauncher\launcherd.exe` → «Выполнять для всех пользователей» при необходимости. Для доступа к GPU лучше запускать от пользователя, а не SYSTEM.
- **nssm**: `nssm install LlamaLauncher C:\LlamaLauncher\launcherd.exe --data-dir C:\LlamaLauncher\data`.

Не запускайте одновременно desktop-приложение и launcherd с одним портом.

## Данные
`%APPDATA%\LlamaLauncher` (БД `launcher.db`, логи, скачанные модели и сборки). Портативный режим: положите рядом с exe файл `portable` (или `portable.txt`) — данные будут в `data\` рядом. Либо `--data-dir <путь>` / переменная `LLL_DATA_DIR`.
Бэкап/восстановление — **Настройки → Бэкап** (zip; файлы моделей не включаются).

## Тесты
```bash
cargo test -p lll-core
cargo build -p launcherd && bash tests/e2e.sh      # Linux/WSL, нужен python3
```
e2e проверяет: сканирование сборки и схемы, модели, пресеты, оценку памяти, роутер с автозапуском, стриминг и reasoning в реальном времени, логи, TTL-выгрузку, подсказку при OOM, группы алиасов, импорт команды, удалённый вход (JWT, ключи, права), экспорт, бэкап, отдачу UI.

## Что стоит проверить на реальном железе
- Мониторинг VRAM: NVIDIA — NVML; AMD/Intel — счётчики DXGI/PDH (системные). Калибровка пресета («Калибровать» при запущенной модели) уточняет оценку по факту.
- Установка сборок с GitHub (имена ассетов у форков бывают нестандартными — тогда добавьте сборку папкой).
- ROCm/Vulkan: выбор устройств идёт через `--device` по списку `--list-devices`.
