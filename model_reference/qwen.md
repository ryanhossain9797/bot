# Qwen family — prompt & tool-calling format

Reference for how Qwen models expect their prompts, so we can drive them natively
(ChatML envelope, reasoning channel, tool calling). Notes here are the **ground truth
extracted from the actual GGUF we ship** — not from docs — because that's exactly what
llama.cpp applies at runtime.

> **Heads-up on variation:** the tool-call wire format is NOT identical across Qwen
> releases. Stock **Qwen2.5** emits JSON inside `<tool_call>` (`{"name":..,"arguments":..}`).
> Our model (**Qwen3.6-27B**, a reasoning model) uses an **XML `<function=…>` format** and a
> `<think>` reasoning channel. Always confirm against the model's embedded template.

---

## Model in use: Qwen3.6-27B-Q4_K_M

- File: `chatbot/models/Qwen3.6-27B-Q4_K_M.gguf` (also in LM Studio cache).
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

> **Parsing is the harness's job.** llama.cpp's *server* parses `<tool_call>` for you; the
> embedded library (what `llama-cpp-2` wraps) does not — we parse the `<function=…><parameter=…>`
> blocks ourselves in Rust.

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
- **Tool calls:** parse XML `<function=…>`, not the current `{"GetWeather":"..."}` JSON.
- **Open question:** does `llama-cpp-2` expose `llama_chat_apply_template`? If not, hand-render
  the ChatML above (structure is simple). Tool-call parsing is ours either way.
```
