# Architecture Overview

High-level module map of the rust-tiny-claw Harness runtime.

```mermaid
graph TB
    subgraph Binaries["Entry Points"]
        CLI["tiny-claw\nCLI mode"]
        FEISHU_BIN["tiny-claw-feishu\nFeishu bot mode"]
    end

    subgraph Engine["engine · ReAct Main Loop"]
        REACT["ReAct Loop\n(Think → Act → Observe)"]
        PLAN["plan_mode\nPlan Mode"]
    end

    subgraph AgentRuntime["agent_runtime · Subagents"]
        SUPERVISOR["Supervisor\nTask delegation"]
        TEMPLATES["Prompt Templates"]
    end

    subgraph ContextEngine["context_engine · Context Management"]
        SKILLS["skills.rs\nDynamic skill loading"]
        COMPACTION["compaction.rs\nContext compaction"]
        RECOVERY["recovery.rs\nError recovery injection"]
        REMINDER["reminder.rs\nSystem reminders"]
    end

    subgraph Provider["provider · Model Adapters"]
        PROVIDER_TRAIT["Provider trait\nModel boundary"]
        CLAUDE["claude_compatible\nClaude / Anthropic"]
        OPENAI["openai_compatible\nOpenAI-compatible API"]
        SSE["sse.rs\nStreaming parser"]
    end

    subgraph Tools["tools · Tool Set"]
        REGISTRY["registry.rs\nRegistration & dispatch"]
        BASH["bash.rs"]
        EDIT["edit.rs\nFuzzy-match edit"]
        READ["read_file.rs"]
        WRITE["write_file.rs"]
        GREP["grep.rs"]
        LOAD_SKILL["load_skill.rs"]
        PERM["permission.rs\nDangerous-command middleware"]
    end

    subgraph Memory["memory · Persistent State"]
        SESSION["session.rs\nSession isolation"]
        FILE_MEM["file.rs\nFilesystem memory"]
        MANAGER["manager.rs\nMemory & todo management"]
    end

    subgraph Integrations["integrations · External"]
        FEISHU["feishu/\nEvent stream\nHuman approval"]
    end

    subgraph Telemetry["telemetry · Observability"]
        TELEM["Telemetry\nIn-memory totals"]
        TELEMETRY_PROVIDER["TelemetryProvider\nLLM call timing & usage"]
        TELEMETRY_TOOLS["TelemetryToolMiddleware\nTool call timing"]
    end

    CLI --> REACT
    FEISHU_BIN --> FEISHU
    FEISHU --> REACT

    REACT --> TELEMETRY_PROVIDER
    TELEMETRY_PROVIDER --> PROVIDER_TRAIT
    PROVIDER_TRAIT --> CLAUDE & OPENAI
    CLAUDE --> SSE
    OPENAI --> SSE

    REACT --> REGISTRY
    REACT --> ContextEngine
    REACT --> AgentRuntime
    REACT --> Memory
    REACT --> TELEM

    REGISTRY --> BASH & EDIT & READ & WRITE & GREP & LOAD_SKILL
    REGISTRY --> PERM
    REGISTRY --> TELEMETRY_TOOLS
    TELEMETRY_PROVIDER --> TELEM
    TELEMETRY_TOOLS --> TELEM

    PLAN --> REACT
    SUPERVISOR --> REACT
    SUPERVISOR --> TELEMETRY_PROVIDER
```

## Layer Summary

| Layer | Module | Responsibility |
|---|---|---|
| Entry | `bin/` | CLI arg parsing; Feishu HTTP server bootstrap |
| Engine | `engine/`, `plan_mode` | ReAct loop orchestration; Plan Mode |
| Subagents | `agent_runtime/` | Subagent lifecycle, supervisor, prompt templates |
| Context | `context_engine/` | Skill loading, context compaction, error recovery, system reminders |
| Provider | `provider/` | Model-agnostic trait; Claude and OpenAI-compatible adapters; SSE parsing |
| Tools | `tools/` | Tool trait, registry, dispatch; permission and telemetry middleware |
| Memory | `memory/` | File-backed session state, working memory, todo management |
| Integration | `integrations/feishu/` | Feishu event stream; human-approval webhook |
| Telemetry | `telemetry/` | LLM token usage aggregation; LLM and tool elapsed-time totals |
