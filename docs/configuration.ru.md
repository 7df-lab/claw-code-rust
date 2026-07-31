# Конфигурация

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` - рекомендуемый путь настройки. Для ручной конфигурации Devo
объединяет настройки в таком порядке:

1. Встроенные значения по умолчанию
2. `DEVO_HOME/config.toml` - пользовательская конфигурация, по умолчанию
   `~/.devo/config.toml` на macOS/Linux и
   `C:\Users\yourname\.devo\config.toml` на Windows
3. `<workspace>/.devo/config.toml` - конфигурация уровня проекта
4. CLI flags

Учетные данные хранятся отдельно в `DEVO_HOME/auth.json`; `config.toml` должен
ссылаться на credential id, а не хранить API key напрямую.

Минимальная структура (встроенная модель + provider binding):

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

Важное разделение:

- `model_slug` выбирает локальные метаданные модели Devo по slug.
- `provider` в binding выбирает запись подключения `[providers.<id>]`.
- `request_model` - id модели, отправляемый поставщику.
- `invocation_method` выбирает рабочий протокол поставщика. См.
  [Методы вызова (Invocation methods)](#методы-вызова-invocation-methods).

В метаданных модели тоже есть поле `provider`: оно описывает wire API модели.
`invocation_method` в binding выбирает рабочее подключение; эти значения должны
соответствовать друг другу. API key остается в `auth.json` и подключается через
ссылку `credential` поставщика.

Существующая конфигурация с `model_name` по-прежнему читается. При следующем
сохранении binding Devo запишет поле как `request_model`.

## Свой API key

Devo не хранит API key в `config.toml`. Чтобы подключить свой ключ:

1. Сохраните секрет в пользовательском `DEVO_HOME/auth.json`.
2. Укажите этот credential id в `[providers.<id>].credential` в `config.toml`.

`devo onboard` и потоки provider в Desktop/TUI записывают оба файла за вас.

### Полный пример: кастомная модель + свой API key

Ниже вместе настраиваются кастомная модель DeepSeek (Anthropic Messages),
endpoint поставщика и credential, который хранится только в `auth.json`.

`~/.devo/config.toml` (на Windows: `C:\Users\yourname\.devo\config.toml`):

```toml
[defaults]
model_binding = "deepseek-example"

[model.my-deepseek]
display_name = "DeepSeek V4 Flash"
description = "Custom Anthropic Messages coding model for DeepSeek."
channel = "Custom"
# Wire API, который ожидает модель. Должен совпадать с invocation_method binding.
provider = "anthropic_messages"
context_window = 200000
effective_context_window_percent = 95
max_tokens = 8192
temperature = 0.2
reasoning_capability = { togglewithlevels = ["high", "max"] }
reasoning_implementation = "request_parameter"
base_instructions = "(optional) You are Devo, a coding agent."
input_modalities = ["text", "image"]

[providers.deepseek]
enabled = true
name = "DeepSeek Anthropic Compatible"
base_url = "https://api.deepseek.com/anthropic"
# Только credential id — секрет лежит в auth.json.
credential = "deepseek_compatible_api_key"
wire_apis = ["anthropic_messages"]

[model_bindings.deepseek-example]
enabled = true
model_slug = "my-deepseek"
provider = "deepseek"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "anthropic_messages"
```

Соответствующий `~/.devo/auth.json` (на Windows:
`C:\Users\yourname\.devo\auth.json`):

```json
{
  "version": 1,
  "credentials": {
    "deepseek_compatible_api_key": {
      "kind": "api_key",
      "value": "sk-deepseek-your-api-key"
    }
  }
}
```

Правила:

- Сейчас поддерживаются только credentials вида `api_key`.
- Credential id должен точно совпадать с `[providers.<id>].credential`.
- Храните `auth.json` в `DEVO_HOME`. Не коммитьте его в репозиторий проекта.
- Workspace `<workspace>/.devo/config.toml` может ссылаться на credential id, но
  значения секретов остаются только в пользовательском `auth.json`.
- Чтобы обновить только ключ, правьте `auth.json`; при том же credential id
  `config.toml` менять не нужно.

## Методы вызова (Invocation methods)

`invocation_method` на model binding и `wire_apis` на provider выбирают, какой
HTTP API использует Devo. Поле `provider` в метаданных модели должно совпадать,
чтобы возможности каталога соответствовали рабочему подключению.

| Значение | Протокол | Типичные endpoints |
| --- | --- | --- |
| `openai_chat_completions` | [OpenAI Chat Completions](https://developers.openai.com/api/reference/chat-completions/overview) | Большинство OpenAI-совместимых шлюзов (DeepSeek, Qwen, Kimi, OpenRouter, многие локальные прокси) |
| `openai_responses` | [OpenAI Responses](https://developers.openai.com/api/reference/responses/overview) | Сервисы с Responses API |
| `anthropic_messages` | [Anthropic Messages](https://platform.claude.com/docs/en/api/messages) | Anthropic-совместимые Messages endpoints |

## Метаданные и пользовательские модели

Настройте метаданные в пользовательском или workspace `config.toml` в разделе
`[model.<slug>]`. Для встроенного slug это частичное переопределение: пропущенные
поля сохраняют встроенные значения. Новый slug создает модель с безопасными
значениями по умолчанию; подключите ее через `[providers.<id>]` и
`[model_bindings.<id>]`, как в
[полном примере](#полный-пример-кастомная-модель--свой-api-key).

Пример частичного переопределения встроенной модели:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

Точная формула эффективного контекстного окна:
`context_window * effective_context_window_percent / 100`; результат является
доступным модели контекстом и границей автоматической compaction.

Настраиваемые метаданные включают `display_name` (имя в picker), `description`
(поясняющий текст) и `channel` (метка группировки). `context_window` и
`effective_context_window_percent` задают эффективный контекст, а `max_tokens` -
лимит выходных токенов по умолчанию. Параметры sampling: `temperature`
(случайность), `top_p` (nucleus sampling), `top_k` (лимит кандидатов). Wire API
в `provider` - одно из `openai_chat_completions`, `openai_responses` или
`anthropic_messages`. Метаданные reasoning типизированы: `reasoning_capability`
может быть `unsupported`, `toggle`, `{ levels = [...] }` или
`{ togglewithlevels = [...] }`; `reasoning_implementation` - `disabled`,
`request_parameter` или типизированная таблица `model_variant`. Вариант модели
отображает логический выбор reasoning в другой provider-facing model id,
опциональный effective effort и опциональное extra request body;
`default_reasoning_effort` выбирает effort по умолчанию. `input_modalities`
принимает `text` и `image`; `truncation_policy` задает лимит байт или токенов для
слишком больших tool results; `supports_image_detail_original` включает
оригинальную детализацию изображений.

Если `base_instructions` опущен, встроенная модель сохраняет встроенные
инструкции, а кастомная использует инструкции Devo по умолчанию. Явная пустая
строка (`base_instructions = ""`) означает отсутствие базовых инструкций.

Устаревший скаляр `model = "slug"` по-прежнему читается. Поскольку
`[model.<slug>]` занимает пространство имен таблицы `model`, новая конфигурация
должна выбирать активное подключение через `[defaults].model_binding`.

### Настройки TUI

Ключевые поля верхнего уровня в `DEVO_HOME/config.toml` также хранят UI-настройки:

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` выбирает цветовую тему TUI (также через `/theme`).
- `collapse_reasoning` управляет отображением reasoning (также через
  `/show-reasoning`):
  - `true` (по умолчанию): при streaming показывать только последние 3 строки;
    после завершения короткие рассуждения оставлять полностью, длинные сворачивать
    в одну строку `Thought · …` (полный текст доступен по Ctrl+T).
  - `false`: показывать полный reasoning и во время streaming, и после него.

### Миграция с `models.json`

Старые `~/.devo/models.json` и `<workspace>/.devo/models.json` игнорируются.
Вручную скопируйте нужные поля в `[model.<slug>]` пользовательского или
workspace `config.toml`, затем добавьте или сохраните соответствующие provider и
binding. API key храните в `auth.json` и ссылайтесь на него через
`[providers.<id>].credential`.

## MCP-серверы

Devo подключается к серверам [Model Context Protocol](https://modelcontextprotocol.io/),
настроенным в пользовательском или workspace `config.toml` в разделе `[mcp]`.
Каждый сервер - это одна запись в массиве `servers`, а его таблица `transport`
определяет способ подключения. Поддерживаются транспорты `stdio`,
`streamable_http` и устаревший `sse`.

Пример stdio:

```toml
[mcp]
auto_start = true
refresh_on_config_reload = true

[[mcp.servers]]
id = "filesystem"
display_name = "Filesystem"
enabled = true
startup_policy = "lazy" # eager | lazy | manual
trust_policy = "user" # user | workspace | untrusted
allowed_capabilities = ["tools", "resources", "prompts"]
roots_policy = "workspace" # none | workspace | custom

[mcp.servers.transport]
kind = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
# cwd = "/path/to/workdir"
# env = { MY_VAR = "value" }
# env_vars = ["HOME", "PATH"]
```

Streamable HTTP с bearer token:

```toml
[[mcp.servers]]
id = "github"
display_name = "GitHub"
startup_policy = "lazy"

[mcp.servers.transport]
kind = "streamable_http"
url = "https://api.githubcopilot.com/mcp/"
auth = { kind = "bearer_token", token = "replace-me" }
http_headers = { "X-Custom" = "static-value" }
env_http_headers = { "Authorization" = "GITHUB_TOKEN" }
```

Устаревший SSE-транспорт:

```toml
[mcp.servers.transport]
kind = "sse"
url = "https://example.com/mcp/sse"
```

Примечания к полям:

- `auto_start` и `refresh_on_config_reload` по умолчанию равны `true`.
- `startup_policy` управляет запуском включенного сервера: `eager` - при
  bootstrap, `lazy` - при первом использовании, `manual` - только по явному
  запросу.
- Для stdio `env` задает литеральные значения, а `env_vars` - список имен
  переменных, наследуемых из локального окружения; `{ name = "X",
  source = "remote" }` для stdio не поддерживается.
- Для HTTP-транспортов `http_headers` задает литеральные заголовки, а
  `env_http_headers` сопоставляет имя заголовка с переменной окружения,
  поставляющей его значение.
- Пустой `allowed_capabilities` означает отсутствие ограничений. Сейчас рантайм
  в основном работает с `tools`; чтение resources еще не подключено.
- `output_limits` задает `max_tool_output_bytes` (по умолчанию 1 MiB) и
  `max_resource_bytes` (по умолчанию 10 MiB).
- Верхнеуровневый `mcp_oauth_credentials_store` принимает `auto` (по умолчанию),
  `file` или `keyring` и выбирает место хранения OAuth-credentials.
- Предпочитайте заголовки или значения из переменных окружения, а не жестко
  зашитые token в `config.toml`. Поле `auth_ref` есть в каждой записи сервера,
  но пока не подключено к рантайму.

Поведение при слиянии: `[mcp]` сливается по полям, как другие таблицы, но
`servers` - это массив. Поэтому список `[[mcp.servers]]` уровня проекта заменяет
пользовательский список целиком, а не сливает по `id`.

Проверить конфигурацию можно в TUI командой `/mcp list`.
