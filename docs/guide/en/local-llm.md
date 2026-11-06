# Local models

The assistant does not have to run in the cloud: Glafira answers any MCP client the same way, and the client can talk to a model in your network or on your own computer. This chapter covers where the model can run, what a local model can do on Glafira tasks, how to set it up and connect it, and how to check the result. How the assistant's access to databases works is described in the [LLM assistants (MCP)](llm-mcp.md#how-it-works) chapter.

## Where the model runs {#where}

1. **A cloud model** (Claude, GPT and others): the strongest, but database diagnostics — topology, reports, query texts and plans — leave your network. Glafira masks literals in query texts ([Masking](llm-mcp.md#masking)), but object, role and application names stay. Many companies forbid this.
2. **A corporate model server** inside your network: the data stays inside, and administrators choose the model and hardware. If there is no such server yet, this is the next step.
3. **A model on the engineer's computer**: when the cloud is forbidden and there is no corporate server yet. The data does not leave the computer, but the model is weaker than a cloud one and slower.

The rest of the chapter is about the third option; a client for the second is set up the same way, only the server address changes.

## What a local model can do {#capabilities}

Tested on a Mac mini with an M4 Pro and 48 GB of memory, models in Ollama quantized to 4 bits with a 64K-token context (32K for Gemma 4 31B). The test stand: a three-node Patroni cluster, a server with an orders database and a server under a pgbench load. Six typical tasks, three runs of each model:

1. What is happening on the cluster — name the leader and the replicas.
2. Who holds the xmin horizon — name the session and quote its query; the literals in the quote must stay masked.
3. Are there locks, and which session is the root.
4. What loads the server over the last 15 minutes, when Glafira has no history for that window.
5. The plan of a query when no database is named.
6. Would an index help — a hypothesis check with hypopg without creating the index.

| Model (4-bit) | Correct answers | Generation, typically | Tasks 1–3, 5, 6 | Top over 15 minutes |
|---|---|---|---|---|
| Qwen3.6-35B-A3B | 17 of 18 | 60–80 tokens/s | 6–30 s | 1.5–2 min |
| Gemma 4 26B | 18 of 18 | 35–50 tokens/s | 7–42 s | 1.5 min |
| Qwen3.8-27B | 18 of 18 | 15–20 tokens/s | 23–100 s | 5–7 min |
| Gemma 4 31B | 16 of 18 | 7–15 tokens/s | 47–119 s | 4–7.5 min |

Generation is the number of answer tokens per second of model time, including reading the context; the first question after the model is loaded into memory is slower. Qwen3.6-35B-A3B and Gemma 4 26B answer ordinary questions in tens of seconds. The dense Qwen3.8-27B and Gemma 4 31B use all of their parameters for every token and answer in minutes, and up to 7 minutes on a task with several large tool answers. For everyday work on a Mac, Qwen3.6-35B-A3B and Gemma 4 26B fit: Gemma made no mistakes in these tasks, Qwen is faster.

All mistakes were in the fourth task. Once Qwen3.6 titled its answer "over the last 15 minutes" although it mixed 30 seconds of observation with statistics since the reset. Once Gemma 4 31B answered "recently" without naming the period, and another time it presented statistics since the reset as the current load. In a trial run without load, Qwen3.6 once used another connection's database, got an empty top and decided that the server was idle. The models fixed wrong tool arguments themselves on the next attempt (a name for a hypothetical index, an empty `queryId`, observing for longer than 50 seconds): Glafira answers with a clear error, but this costs an extra step. A script checked the answers by key signs; disputed ones were reread by hand.

No answer revealed the original values of masked literals, and no model changed anything on the server: the gateway does not allow it. The models erred only in their conclusions — check them against the MCP journal. The hardest task is the fourth: the model has to notice that there is no 15-minute window and say honestly which period its statistics cover.

## Hardware and models {#hardware}

On ordinary hardware, MoE models with few active parameters (3–4 billion, for example Qwen3.6-35B-A3B) work best: they are several times faster than dense models of the same size. The context should be at least 32K tokens, better 64K.

| Platform | Model server | What fits |
|---|---|---|
| macOS, Apple Silicon, 48 GB or more | Ollama | Qwen3.6-35B-A3B, Gemma 4 26B (tested) |
| macOS, Apple Silicon, 32 GB | Ollama | models of about 20 billion parameters; 35B-A3B is at the limit |
| Linux, NVIDIA with 24 GB of video memory | vLLM | 35B-A3B in 4 bits with a smaller context |
| Windows, NVIDIA | Ollama; vLLM only in WSL2 | as on Linux |
| x86 without a GPU, 32–64 GB DDR5 | Ollama | MoE models only, slowly |

A Mac with an Intel processor is not suitable for local models.

## Model server {#server}

The server must provide an OpenAI-compatible API with tool calling. On a Mac, [Ollama](https://ollama.com) is the simplest: install the app, download the model and create a variant of it with a 64K-token context — by default Ollama uses a smaller context, and Glafira's tool descriptions together with the answers do not fit:

```bash
ollama pull qwen3.6:35b-a3b
printf 'FROM qwen3.6:35b-a3b\nPARAMETER num_ctx 65536\n' > Modelfile
ollama create qwen3.6-64k:35b-a3b -f Modelfile
```

The client needs the address `http://127.0.0.1:11434/v1`, the model `qwen3.6-64k:35b-a3b` and any non-empty API key. Ollama listens only on `127.0.0.1`, so the model cannot be reached from other computers.

After 5 minutes of idling Ollama unloads the model from memory, and the next question waits for it to load again. To keep the model in memory longer, set the `OLLAMA_KEEP_ALIVE` environment variable, for example to `1h`.

## Clients {#clients}

Two command-line clients were tested. In both, turn off the built-in shell, file-editing, file-reading and web tools: otherwise the model can bypass Glafira and reach the database or the internet by itself, or even wander off searching files instead of answering. Glafira shows a new client in the approval dialog under its name.

**Qwen Code** (open source, Apache 2.0). Its settings are in `~/.qwen/settings.json`:

```json
{
  "env": { "LOCAL_LLM_API_KEY": "ollama" },
  "modelProviders": { "openai": [ {
    "id": "qwen3.6-64k:35b-a3b",
    "envKey": "LOCAL_LLM_API_KEY",
    "baseUrl": "http://127.0.0.1:11434/v1",
    "generationConfig": { "contextWindowSize": 65536,
      "samplingParams": { "temperature": 0.2, "max_tokens": 8192 } } } ] },
  "security": { "auth": { "selectedType": "openai" } },
  "model": { "name": "qwen3.6-64k:35b-a3b" },
  "privacy": { "usageStatisticsEnabled": false },
  "general": { "enableAutoUpdate": false },
  "telemetry": { "enabled": false },
  "tools": { "approvalMode": "default",
    "disabled": ["run_shell_command", "edit", "write_file", "web_fetch", "web_search", "agent",
      "read_file", "glob", "grep_search", "list_directory", "notebook_edit"] },
  "mcpServers": { "glafira-pg": {
    "command": "/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg",
    "args": ["mcp"], "timeout": 120000, "trust": true, "alwaysLoadTools": true } }
}
```

`envKey` is the name of the variable with the key, not the key itself. Without `trust`, Qwen Code asks for confirmation before every Glafira call, even when file edits are approved automatically; if folder trust checking is on in Qwen Code, `trust` works only in a folder you trust. `alwaysLoadTools` gives the model Glafira's tools right away, without an extra tool-search step. In Glafira the client is called `qwen-cli-mcp-client-glafira-pg`. If other MCP servers are configured in Qwen Code, start it with `--allowed-mcp-server-names glafira-pg`, so that the model sees only Glafira.

**Claude Code** works with Ollama directly: Ollama also answers in the Anthropic format. Set the environment variables and add Glafira with the command from the [[ui:settings.mcp.connect.title]] block:

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:11434
export ANTHROPIC_AUTH_TOKEN=ollama
export ANTHROPIC_MODEL=qwen3.6-64k:35b-a3b
export ANTHROPIC_DEFAULT_OPUS_MODEL=qwen3.6-64k:35b-a3b
export ANTHROPIC_DEFAULT_SONNET_MODEL=qwen3.6-64k:35b-a3b
export ANTHROPIC_DEFAULT_HAIKU_MODEL=qwen3.6-64k:35b-a3b
export CLAUDE_CODE_SUBAGENT_MODEL=qwen3.6-64k:35b-a3b
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
claude --disallowedTools "Bash Edit Write WebFetch WebSearch Task Read Glob Grep NotebookEdit"
```

`ANTHROPIC_BASE_URL` goes without `/v1`. `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` turns off updates, telemetry and error reports. No Anthropic account sign-in is needed. In Glafira the client is called `claude-code`.

Both clients were tested on three tasks from the [table above](local-llm.md#capabilities) — the cluster, the xmin horizon and the query plan — with the `qwen3.6-64k:35b-a3b` model in Ollama, three runs each. The clients ran without a window and with the same built-in tools turned off: Qwen Code with calls approved automatically (`--approval-mode yolo`), Claude Code with Glafira's tools allowed (`--allowedTools`):

| Client | Correct answers | Cluster | xmin horizon | Query plan |
|---|---|---|---|---|
| Qwen Code 0.26.5 | 9 of 9 | 35–62 s | 28–47 s | 23–28 s |
| Claude Code 2.1.318 | 9 of 9 | 20–48 s | 27–72 s | 18–32 s |

In all runs Claude Code called only Glafira's tools and its own tool search. In the first check its file reading and search were not turned off, and in one run out of nine the model spent 25 minutes searching its own conversation log for earlier answers instead of calling Glafira, until the run was stopped. If the client stays silent for a long time, stop the question and ask it again.

## How to ask {#tips}

- Name the connection as it is named in Glafira: "on billing", "on the demo cluster".
- Ask for something specific: "top 5 queries", "the plan of this query" — the tool answer is shorter, and the model answers faster.
- Glafira has a "last 15 minutes" window only if the connection's [[ui:diag.tools.queries]] tab is open and has collected history. Open it in advance or ask about the statistics since the reset.
- Do not clear the conversation between related questions. Ollama keeps the already processed beginning of the conversation and reads only the new part, while after clearing the model has to find the connections and load the tool descriptions again.
- If the assistant asks which database to plan the query in, name it; by default Glafira uses the connection's database.

## Checking {#check}

1. Connect the client and approve it in Glafira's dialog.
2. Open one connection to the assistant, preferably a test one, with the [[ui:connections.form.llmLevels.diagnostics]] level.
3. Ask three or four typical questions: "What is happening on cluster …?", "Who holds the xmin horizon?", "Show the plan of this query".
4. In [[ui:settings.mcp.audit.title]], check which tools the model called and with what result, and compare its answer with the data in Glafira.
5. Check masking: query quotes should contain `$1`, `$2`… instead of literals.
6. Turn off the network and repeat one question: the answer should come without the internet.

## Limitations {#limits}

- A local model answers several times slower than a cloud one, and dense models on a Mac are slower still (see the [table](local-llm.md#capabilities)).
- While the model runs, the computer has less memory left: on a Mac with 48 GB, a 35-billion-parameter model with a 64K context takes about 30 GB in Ollama, and Glafira, a browser and Docker next to it can push the system into swap.
- The model can be wrong in its conclusions. Glafira does not let it change anything in the databases, but the decisions are yours.
