---
artifact_id: L2-DES-MODEL-002
revision: 1
status: Draft
active_baseline: no
supersedes: L2-DES-MODEL-001-model-provider-binding
superseded_by:
owner: Assistant
last_updated: 2026-09-02
---

# L2-DES-MODEL-002 - Provider/model JSON catalog

## Decision

Devo has one git-tracked provider/model directory and layered JSON overlays:

- `crates/core/providers.json` is the built-in directory shipped with Devo.
- `<DEVO_HOME>/providers.json` is the user connection and selection overlay.
- `<workspace>/.devo/providers.json` is the project overlay.
- provider `credential` stores the id of a user-scoped auth entry.
- `<DEVO_HOME>/auth.json` is the only durable location for provider API keys.

The embedded directory entries are provider templates. They are not user
Connections and are never removed by normal configuration actions. Confirming
an unconnected built-in template creates a user Connection in the user
`providers.json`; the template's name and Base URL remain read-only in the
onboarding flow, while the API key is entered once and stored in `auth.json`.
Selecting an already connected built-in template goes directly to model
selection, so its API key and Base URL are not edited there. Disconnecting a
Connection removes its user overlay and any unshared credential, while leaving
the directory template available. Custom providers are Connections whose
identity, endpoint, protocol, models, and credential are user-owned.

The JSON file follows the useful part of opencode's design: provider records own
their nested model records, and a model is addressed as `provider/model`.

```json
{
  "model": "local/my-model",
  "provider": {
    "local": {
      "name": "Local Gateway",
      "base_url": "http://127.0.0.1:8000/v1",
      "credential": "local_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "my-model": {
          "name": "My Model",
          "context_window": 131072
        }
      }
    }
  }
}
```

The matching `<DEVO_HOME>/auth.json` entry contains the secret:

```json
{
  "version": 1,
  "credentials": {
    "local_api_key": {
      "kind": "api_key",
      "value": "sk-local-key"
    }
  }
}
```

## Rationale

Provider/model identity is a composite key. A model id is only meaningful in
the provider namespace that serves it. Making the provider map key and model
map key carry that identity removes three duplicated names from user config:
`model_slug`, `model_name`, and `model_id`. A display `name` is optional and is
never used for lookup. Model descriptions are not part of the canonical file;
the picker can use the name and capability metadata already present in the
directory.

The runtime still has compatibility projection types while the Native
protocol and old TOML readers are migrated. Those types are implementation
details, not a second user-facing persistence format.

## Merge and resolution

Provider JSON overlays merge in this order:

```text
built-in directory
        + user providers.json
        + workspace .devo/providers.json
        ↓
effective provider/model directory
```

For one provider or model record, only fields present in the higher-priority
overlay replace lower-priority values. A new model record automatically creates
a custom model. A repeated provider/model record partially overrides a built-in
model. The top-level `model` field is the active `provider/model` reference.

The selected model's provider record supplies the endpoint, credential reference,
headers, enabled state, and wire API. The model may override the wire API and
stores only capability/request metadata needed by the runtime. The credential
reference is resolved from user-scoped `auth.json`; provider JSON never stores
the API key. The git-tracked directory contains no user secrets.

## Migration

Old provider TOML is read as a compatibility input. Provider flows write the
new JSON file and user-scoped `auth.json`, then remove provider/model binding
tables from `config.toml` while preserving unrelated application settings.
Existing `model_name` is accepted only by the legacy TOML reader. New JSON never
writes binding ids, aliases, or duplicated model identifiers.

## Scope

This design changes persistence and catalog resolution. It does not expand the
wire adapter set: the current OpenAI Chat Completions, OpenAI Responses, and
Anthropic Messages adapters remain the supported transports. External protocol
adapters continue to consume the Native projection and contain no provider
selection business logic.
