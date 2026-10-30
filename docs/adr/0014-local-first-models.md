# ADR-0014: Local-first models, with cloud LLMs as an option

- Status: Accepted
- Date: 2026-10-30

## Context

Jarvis MT sends questions and the data they need (prices, positions, deal history) to a language model. It also turns speech into text and text into speech. Cloud services are convenient, but they send the user's trading data and voice to a third party and need a paid key. The default configuration should work on a typical trader's PC without an account, and cloud providers should remain available for users who want them.

## Decision

- The default LLM provider is **Ollama** running `qwen3.5:9b` locally (`llm.default = "local"`). Anthropic and OpenAI are configured as optional providers with API keys in `.env`.
- Speech-to-text uses **whisper.cpp through whisper-rs** with the `base.en` model, and text-to-speech uses **Piper** with the `en_US-lessac-medium` voice. Both run locally.
- Model files are fetched by a script (`scripts/fetch-models.ps1`) and kept out of git.

Status: local LLM provider implemented in Sprint 1. Voice models and the fetch script are planned for Sprint 3.

## Consequences

- With the default setup, questions, data and voice stay on the user's PC. Data sent to a cloud provider is subject to that provider's terms.
- Local models need hardware. The configuration comments say the default model fits an 8 GB GPU, and its first reply after a cold start takes about 100 seconds while the model loads.
- Local models are generally less capable than the largest cloud models. The tool-calling tests and the rule that the model must use tools for figures reduce, but do not remove, that risk.
- Users who choose a cloud provider must accept that their chat data leaves the PC.

## Alternatives considered

- **Cloud models only.** It is simplest, but it sends trading data and voice to a third party by default and requires a paid key before the app works.
- **Cloud speech services.** They are accurate, but they send the user's audio off the PC.
- **A larger local model.** It may be more capable but needs more memory than many trading PCs have. The model can be changed in config, so the default is a choice, not a limit.
