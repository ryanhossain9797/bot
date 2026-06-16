# Qwen family — prompt & tool-calling format

Reference for how Qwen models expect their prompts, so we can drive them natively
(ChatML envelope, reasoning channel, tool calling). Notes here are the **ground truth
extracted from the actual GGUF we ship** — not from docs — because that's exactly what
llama.cpp applies at runtime.

> **Heads-up on variation:** the tool-call wire format is NOT identical across Qwen
> releases. Stock **Qwen2.5** emits JSON inside `<tool_call>` (`{"name":..,"arguments":..}`).
> Our model (**Qwen3.6-35B-A3B**, a reasoning model) uses an **XML `<function=…>` format** and a
> `<think>` reasoning channel. Always confirm against the model's embedded template.

---

## Model in use: Qwen3.6-35B-A3B-Q8_0

- File: `chatbot/models/Qwen3.6-35B-A3B-Q8_0.gguf` (also in LM Studio cache).
- Base format: **ChatML**. Turn delimiters `<|im_start|>{role}\n … <|im_end|>\n`.
- **EOS / end-of-turn token: `<|im_end|>`** — this is what stops generation (no grammar cap needed).
- **Reasoning model:** has a `<think> … </think>` channel. `add_generation_prompt` opens
  `<|im_start|>assistant\n<think>\n` by default, so the model reasons *then* answers. Older
  turns' `<think>` blocks are stripped from history; only the most recent assistant turn keeps it.
- Vision tokens exist (`<|vision_start|>` etc.) but we don't use them.

### Roles
| Role | Rendered as |
|---|---|
| `system` | `<|im_start|>system\n{content}<|im_end|>\n` (must be first; tools get prepended here) |
| `user` | `<|im_start|>user\n{content}<|im_end|>\n` |
| `assistant` | `<|im_start|>assistant\n[<think>…</think>]{content}[tool calls]<|im_end|>\n` |
| `tool` | wrapped in `<tool_response>` inside a **user** turn (consecutive tool msgs grouped) |

---

## Tool calling (this model's XML format)

When `tools` are provided, the template injects a system block:

```
<|im_start|>system
# Tools

You have access to the following functions:

<tools>
{"type": "function", "function": {"name": "get_weather", "description": "...", "parameters": {"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}
</tools>

If you choose to call a function ONLY reply in the following format with NO suffix:

<tool_call>
<function=example_function_name>
<parameter=example_parameter_1>
value_1
</parameter>
</function>
</tool_call>

<IMPORTANT>
Reminder:
- Function calls MUST follow the specified format: an inner <function=...></function> block must be nested within <tool_call></tool_call> XML tags
- Required parameters MUST be specified
- You may provide optional reasoning ... BEFORE the function call, but NOT after
- If there is no function call available, answer normally and do not mention function calls
</IMPORTANT>

{optional original system prompt appended here}<|im_end|>
```

- Each tool is rendered as `tool | tojson` (OpenAI-style `{"type":"function","function":{…}}`).
- **The model emits calls as XML, not JSON:**
  ```
  <tool_call>
  <function=get_weather>
  <parameter=city>
  London
  </parameter>
  </function>
  </tool_call>
  ```
  Multi-line and multi-parameter values are first-class. Optional natural-language reasoning
  may appear BEFORE the `<tool_call>`, never after.
- **Tool results** are fed back as a `tool` role → rendered:
  ```
  <|im_start|>user
  <tool_response>
  {"temp_c": 15, "condition": "cloudy"}
  </tool_response><|im_end|>
  ```

> **The binding parses for us.** `llama-cpp-2` 0.1.146 exposes `parse_response_oaicompat`, which
> turns the raw `<tool_call>` output into structured `tool_calls` JSON — no hand-written XML parser
> needed. (See the "Driving it via llama-cpp-2" section below.)

---

## Full raw prompt example (round trip)

Tool registered: `get_weather(city)`. User asks for London.

**1. Prompt sent to model** (`add_generation_prompt=true`):
```
<|im_start|>system
# Tools

You have access to the following functions:

<tools>
{"type": "function", "function": {"name": "get_weather", "description": "Get current weather for a city", "parameters": {"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}
</tools>

If you choose to call a function ONLY reply in the following format with NO suffix:
... (format + <IMPORTANT> block) ...

You are Terminal Alpha Beta.<|im_end|>
<|im_start|>user
What's the weather in London?<|im_end|>
<|im_start|>assistant
<think>
```

**2. Model generates** (stops at `<|im_end|>`):
```
User wants weather in London. Call get_weather.
</think>

<tool_call>
<function=get_weather>
<parameter=city>
London
</parameter>
</function>
</tool_call><|im_end|>
```

**3. Harness executes tool, appends `tool` result, re-renders tail:**
```
<|im_start|>user
<tool_response>
{"temp_c": 15, "condition": "cloudy"}
</tool_response><|im_end|>
<|im_start|>assistant
<think>
```

**4. Model produces final answer:**
```
Got the weather.
</think>

It's 15°C and cloudy in London right now.<|im_end|>
```

---

## Native harness loop (pseudocode)

```
messages = [ {system}, {user} ]
loop:
    prompt = apply_chat_template(messages, tools=TOOLS, add_generation_prompt=true)
    text   = generate(prompt) until <|im_end|>
    reasoning, body = split_think(text)            # strip <think>…</think>
    calls = parse_tool_calls(body)                 # <function=…><parameter=…>
    if calls:
        messages.append({role:"assistant", content:body, tool_calls:calls})
        for c in calls: messages.append({role:"tool", content: execute(c)})
        continue
    else:
        messages.append({role:"assistant", content:body})
        break                                       # final answer to user
```

---

## Driving it via `llama-cpp-2` (v0.1.146) — confirmed working

The Rust binding wraps llama.cpp's `common_chat` machinery, so we do NOT hand-render ChatML or
hand-parse tool calls. Verified end-to-end with the `probe/` crate.

**Render** — get the embedded template, then render with tools:
- `model.chat_template(None)` → `LlamaChatTemplate` (the model's own template).
- `model.apply_chat_template_with_tools_oaicompat(tmpl, &[LlamaChatMessage], tools_json, json_schema, add_gen)`
  — simple path; messages are role+content; **no** `reasoning_format`.
- `model.apply_chat_template_oaicompat(tmpl, &OpenAIChatTemplateParams { messages_json, tools_json,
  reasoning_format, enable_thinking, use_jinja, add_generation_prompt, parse_tool_calls, … })`
  — params path; messages are an OpenAI-style **JSON array string**; supports `reasoning_format`
  ("auto" | "deepseek" | "deepseek-legacy" | "none").

Both return `ChatTemplateResult { prompt, grammar, grammar_lazy, grammar_triggers, parser,
chat_format, generation_prompt, parse_tool_calls, … }`:
- `prompt` — rendered ChatML to feed the model.
- `grammar` (+ `grammar_lazy`, `grammar_triggers`) — auto-generated tool-call grammar, lazily
  triggered by `<tool_call>`: free text flows + stops on EOS, grammar constrains only tool-call
  args. Optional — Qwen emits valid calls without it; apply it to *enforce* well-formed calls.
- `parser` / `chat_format` — consumed by the response parser.

**Parse** — the binding does it: `rendered.parse_response_oaicompat(raw_output, is_partial)` →
an OpenAI-style message JSON:
```json
{"role":"assistant","reasoning_content":"…","content":"","tool_calls":[
  {"type":"function","id":"…","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\"}"}}]}
```
- `tool_calls[].function.arguments` is a JSON **string** (parse again for the object).
- `reasoning_content` is populated ONLY when `reasoning_format` was set during render; otherwise the
  `<think>` block leaks into `content`.
- It is a method on the **`ChatTemplateResult`** (NOT `LlamaModel`) — it needs the `chat_format` /
  `parser` produced during rendering.

## Empirical behavior (probed locally)

- **No BOS** — tokenize with `AddBos::Never`; the prompt starts at `<|im_start|>` (id 248045);
  `<|im_end|>` is id 248046. Special tokens are single ids, not literal characters.
- **Tool-call turns carry NO user-facing message.** Even when explicitly told "tell me what tool you
  called," the model emits *only* the tool call and defers the message to the turn AFTER the tool
  result. With `reasoning_format` set, `content` is **empty** on a tool-call turn (just
  `reasoning_content` + `tool_calls`). The schema allows content + tool_calls together; this model
  doesn't do it in practice → **branch the loop on "are there tool_calls?", not on content.**
- **Pronouns resolve from history** — asked weather "there" after answering "Paris" → it called
  `get_weather(city="Paris")`.
- **Thinking is verbose** and can be mildly degenerate (repeats "Done./Proceeds./✅") even for trivial
  routing. Real latency/token cost per turn — consider `enable_thinking:false` for simple turns.
- **Sampler / system prompt:** LM Studio preset = temp 0.6, top_k 20, top_p 0.95, min_p off,
  **repeat_penalty OFF**, and **no default system prompt**.

---

## How to re-extract the embedded template

The Jinja chat template is stored in GGUF metadata under `tokenizer.chat_template`
(string, ~7.7 KB for this model). It sits after the tokenizer vocab (~11 MB into the file).
Dependency-free extraction:

```python
import struct
buf = open(PATH,'rb').read(300_000_000)      # metadata is near the start
i = buf.find(b"tokenizer.chat_template")
pos = i + len(b"tokenizer.chat_template")
vtype = struct.unpack_from("<I", buf, pos)[0]; pos += 4   # 8 == STRING
slen  = struct.unpack_from("<Q", buf, pos)[0]; pos += 8
print(buf[pos:pos+slen].decode("utf-8"))
```

(`llama.cpp/gguf-py`'s `gguf_dump.py` also works but needs `numpy`.)

---

## Implications for the bot (Phase 2)

- **Stopping:** `<|im_end|>` is the EOS; `is_eog_token` in `agents.rs` already breaks on EOG.
  With the template applied, multiline messages terminate naturally — no GBNF line-cap.
- **Tool results:** use the `tool` role (`<tool_response>`), fixing the current
  "labeled as Assistant:" problem.
- **Thoughts:** the `<think>` channel replaces the hand-rolled `thoughts:` field, hidden from user.
- **Tool calls:** the model emits XML `<function=…>`, but `parse_response_oaicompat` hands us
  structured `tool_calls` — no XML parsing on our side.
- **Rendering + parsing: RESOLVED** — `llama-cpp-2` 0.1.146 renders (with tools + reasoning_format)
  and parses tool calls for us via `apply_chat_template_oaicompat` + `parse_response_oaicompat`.
  No hand-rolled ChatML or XML parser. See "Driving it via llama-cpp-2" above.
```
