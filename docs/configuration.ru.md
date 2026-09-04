# Конфигурация

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` — рекомендуемый путь настройки. Provider и model хранятся в отдельном
`providers.json`, а API key — только в пользовательском `auth.json`. Старые provider/model
поля из `config.toml` автоматически мигрируются при запуске; после успешной миграции старые
поля удаляются.

## Файлы и приоритет

Настройки объединяются в порядке: встроенные значения, `DEVO_HOME/config.toml`,
`DEVO_HOME/providers.json`, `.devo/config.toml` workspace, `.devo/providers.json` workspace,
затем CLI flags. Встроенный каталог, отслеживаемый git, находится в
`crates/core/providers.json`; пользовательские и workspace-файлы являются overlay.

Минимальная форма:

```json
{
  "model": "my-provider/my-model",
  "provider": {
    "my-provider": {
      "name": "My Provider",
      "base_url": "https://api.example.com/v1",
      "credential": "my_provider_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "my-model": { "name": "My Model", "context_window": 131072 }
      }
    }
  }
}
```

Provider id и model id — это ключи map. Единственный стабильный внешний идентификатор
модели — `provider/model`; отдельные binding id, local slug, request model и model description
не нужны.

`model` — основная модель для обычных turn. Необязательный `small_model` используется для
легких фоновых задач, например заголовков сессий. Если он не задан или недействителен, Devo
сначала ищет flash, mini, small или nano в том же provider, затем возвращается к основной модели.

## auth.json

В `providers.json` хранится только credential id. Секрет записывается в пользовательский
`DEVO_HOME/auth.json` и не должен попадать в git:

```json
{
  "version": 1,
  "credentials": {
    "my_provider_api_key": { "kind": "api_key", "value": "sk-your-key" }
  }
}
```

Сейчас поддерживается только `kind: "api_key"`. Значение `credential` должно точно совпадать
с `credentials.<id>`. Не помещайте `apiKey`, `api_key` или сам секрет в `providers.json`.

## Provider templates и Connections

Встроенные provider — это read-only шаблоны из git-каталога, а не подключенные аккаунты.
Подтверждение шаблона создает пользовательский Connection в `providers.json`. У встроенного
шаблона имя, Base URL и wire API нельзя менять в этом потоке. Для смены ключа или endpoint
сначала отключите Connection, затем подключите шаблон заново. У custom provider имя, endpoint,
протокол и model принадлежат пользователю и редактируются.

В уже подключенном provider можно просмотреть сохраненные модели, добавить custom model или
удалить модель. `d`/Delete отключает весь Connection, но не удаляет встроенный шаблон.
Новый ключ в provider map создает custom provider, новый ключ в `models` — custom model.

## Поля provider и model

Provider поддерживает `name`, `base_url`, `credential`, `wire_api`, `enabled`, `headers`,
`options`, `request` и `models`. `headers` — JSON object со строковыми ключами и значениями;
API key должен находиться в `auth.json`.

Ключ model map — это id модели, отправляемый provider. Поддерживаются `name`, `wire_api`,
`context_window`, `effective_context_window_percent`, `max_tokens`, `temperature`, `top_p`,
`top_k`, `input_modalities` (`text`/`image`), `base_instructions`, `enabled`, `priority`,
`family`, `release_date`, `status`, `cost`, `metadata`, `headers`, `options` и `request`.

`reasoning_capability` может быть `unsupported`, `toggle` или
`{"levels":["off","low","high"]}` (включите `off`, чтобы разрешить отключение;
без `off` рассуждение нельзя выключить). Устаревшая форма
`{"toggle_with_levels":[...]}` при чтении мигрирует в `levels` с ведущим `off`.
Значения effort: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`.
`reasoning_implementation` оставлен только для миграции старого TOML; в новом JSON используйте
именованные variants.

Variant — это именованный режим внутри model и может содержать `label`, `disabled`, произвольные
JSON `options`/`request` и строковые `headers`:

```json
{
  "models": {
    "reasoning-model": {
      "variants": {
        "fast": {
          "label": "Fast",
          "options": { "thinking": { "budget": 1024 } },
          "request": { "speed": "fast" },
          "headers": { "X-Mode": "fast" }
        }
      },
      "default_variant": "fast"
    }
  }
}
```

`wire_api` принимает `openai_chat_completions`, `openai_responses` или
`anthropic_messages`. Значение model переопределяет значение provider; по умолчанию используется
`openai_chat_completions`. Встроенный DeepSeek использует официальный Anthropic-compatible endpoint.

## Динамическое обнаружение

Для подключенного Connection Native `provider/discover` использует credential из `auth.json`,
пробует `/models` и `/v1/models`, нормализует ответ и сохраняет модели в пользовательский
`providers.json`. Параметр `{"forceRefresh":true}` отключает короткий cache.

Полная таблица полей, web search/fetch и подробности миграции приведены в
[английском справочнике конфигурации](./configuration.md).
