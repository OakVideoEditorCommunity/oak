#!/usr/bin/env node
// 离线 stub 模型服务：OpenAI 兼容 /v1/chat/completions。
//
// 用途：在没有真实 LLM key 的机器/CI 上，让 pi 的完整 agent 循环（系统提示 +
// AGENTS.md 上下文 + MCP 工具调用 + 工具结果回喂）真实跑一遍。模型输出不是
// 推理出来的，而是按 fixture 脚本逐请求回放——所以验证结果必须标注「未用真实
// LLM」（见 README「已知限制」）。
//
// 用法：
//   node tests/stub-model-server.mjs --port 18731 --script tests/fixtures/smoke.json \
//        --log /tmp/oak-ai/stub-requests.jsonl
//
// 脚本格式（steps 与「进入模型的请求」一一对应，按序取用）：
//   { "steps": [
//       { "say": "我要先建工程" },                       // 纯文本回复
//       { "calls": [ { "name": "mcp__oak__project_open",
//                      "arguments": { "name": "smoke", "fps": 30 } } ] },  // 一次可多调用
//       ...
//     ],
//     "final": { "say": "（脚本已走完）" } }
//
// 每次请求都会原样（含 tools/messages 摘要）追加一行到 --log，作为「pi 真的把
// MCP 工具面交给了模型」的取证。

import http from "node:http";
import fs from "node:fs";
import path from "node:path";

function parseArgs(argv) {
	const out = { port: 0, script: "", log: "", host: "127.0.0.1" };
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i];
		if (a === "--port") out.port = Number(argv[++i]);
		else if (a === "--script") out.script = argv[++i];
		else if (a === "--log") out.log = argv[++i];
		else if (a === "--host") out.host = argv[++i];
		else throw new Error(`unknown argument: ${a}`);
	}
	if (!out.script) throw new Error("--script is required");
	return out;
}

const args = parseArgs(process.argv.slice(2));
const script = JSON.parse(fs.readFileSync(args.script, "utf8"));
const steps = Array.isArray(script.steps) ? script.steps : [];
const finalStep = script.final ?? { say: "脚本已走完（stub 无更多步骤）" };

let cursor = 0;
let callSeq = 0;

function logLine(obj) {
	if (!args.log) return;
	fs.mkdirSync(path.dirname(args.log), { recursive: true });
	fs.appendFileSync(args.log, JSON.stringify(obj) + "\n");
}

function nextStep() {
	if (cursor < steps.length) return steps[cursor++];
	return finalStep;
}

function summarizeRequest(body) {
	return {
		model: body.model,
		stream: !!body.stream,
		message_count: Array.isArray(body.messages) ? body.messages.length : 0,
		// 系统提示是否含 AGENTS.md 链的标记（由行首 marker 断言）
		system_has_context_marker:
			Array.isArray(body.messages) &&
			body.messages.some(
				(m) =>
					typeof m.content === "string" &&
					m.content.includes("OAK-AGENT-CONTEXT"),
			),
		// 工具面取证：pi 交给模型的工具名（截断到前 40 个）
		tool_names: Array.isArray(body.tools)
			? body.tools.map((t) => t?.function?.name ?? t?.name).slice(0, 40)
			: [],
		tool_count: Array.isArray(body.tools) ? body.tools.length : 0,
		// 最近一条工具结果（证明工具调用结果真的回喂给模型）
		last_tool_result: (() => {
			const msgs = body.messages;
			if (!Array.isArray(msgs)) return null;
			for (let i = msgs.length - 1; i >= 0; i--) {
				if (msgs[i]?.role === "tool") {
					const c = msgs[i].content;
					const s = typeof c === "string" ? c : JSON.stringify(c);
					return s.slice(0, 600);
				}
			}
			return null;
		})(),
	};
}

function sse(res, payload) {
	res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

function chunkBase(body) {
	return {
		id: `chatcmpl-stub-${Date.now()}`,
		object: "chat.completion.chunk",
		created: Math.floor(Date.now() / 1000),
		model: body.model ?? "stub",
	};
}

function respondNonStream(res, body, step) {
	const calls = step.calls ?? [];
	const message = { role: "assistant", content: step.say ?? null };
	if (calls.length > 0) {
		message.tool_calls = calls.map((c) => {
			const id = `call_stub_${++callSeq}`;
			return {
				id,
				type: "function",
				function: {
					name: c.name,
					arguments: JSON.stringify(c.arguments ?? {}),
				},
			};
		});
	}
	const payload = {
		id: `chatcmpl-stub-${Date.now()}`,
		object: "chat.completion",
		created: Math.floor(Date.now() / 1000),
		model: body.model ?? "stub",
		choices: [
			{
				index: 0,
				message,
				finish_reason: calls.length > 0 ? "tool_calls" : "stop",
			},
		],
		usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
	};
	res.writeHead(200, { "content-type": "application/json" });
	res.end(JSON.stringify(payload));
}

function respondStream(res, body, step) {
	res.writeHead(200, {
		"content-type": "text/event-stream",
		"cache-control": "no-cache",
		connection: "keep-alive",
	});
	const base = chunkBase(body);
	sse(res, {
		...base,
		choices: [{ index: 0, delta: { role: "assistant", content: "" }, finish_reason: null }],
	});
	if (step.say) {
		sse(res, {
			...base,
			choices: [{ index: 0, delta: { content: step.say }, finish_reason: null }],
		});
	}
	const calls = step.calls ?? [];
	calls.forEach((c, i) => {
		sse(res, {
			...base,
			choices: [
				{
					index: 0,
					delta: {
						tool_calls: [
							{
								index: i,
								id: `call_stub_${++callSeq}`,
								type: "function",
								function: { name: c.name, arguments: "" },
							},
						],
					},
					finish_reason: null,
				},
			],
		});
		sse(res, {
			...base,
			choices: [
				{
					index: 0,
					delta: {
						tool_calls: [
							{ index: i, function: { arguments: JSON.stringify(c.arguments ?? {}) } },
						],
					},
					finish_reason: null,
				},
			],
		});
	});
	sse(res, {
		...base,
		choices: [
			{ index: 0, delta: {}, finish_reason: calls.length > 0 ? "tool_calls" : "stop" },
		],
		usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
	});
	res.write("data: [DONE]\n\n");
	res.end();
}

const server = http.createServer((req, res) => {
	if (req.method === "GET" && req.url.startsWith("/v1/models")) {
		res.writeHead(200, { "content-type": "application/json" });
		res.end(
			JSON.stringify({
				object: "list",
				data: [{ id: "oak-replay", object: "model", owned_by: "stub" }],
			}),
		);
		return;
	}
	if (req.method !== "POST" || !req.url.startsWith("/v1/chat/completions")) {
		res.writeHead(404, { "content-type": "application/json" });
		res.end(JSON.stringify({ error: { message: "not found" } }));
		return;
	}
	let raw = "";
	req.on("data", (d) => (raw += d));
	req.on("end", () => {
		let body;
		try {
			body = JSON.parse(raw || "{}");
		} catch {
			res.writeHead(400, { "content-type": "application/json" });
			res.end(JSON.stringify({ error: { message: "bad json" } }));
			return;
		}
		const step = nextStep();
		logLine({
			ts: new Date().toISOString(),
			step_index: cursor,
			request: summarizeRequest(body),
			reply: { say: step.say ?? null, calls: (step.calls ?? []).map((c) => c.name) },
		});
		if (body.stream) respondStream(res, body, step);
		else respondNonStream(res, body, step);
	});
});

server.listen(args.port, args.host, () => {
	const addr = server.address();
	// stdout 只打印端口，方便脚本用 $(...) 取
	process.stdout.write(`${addr.port}\n`);
});

for (const sig of ["SIGTERM", "SIGINT"]) {
	process.on(sig, () => server.close(() => process.exit(0)));
}
