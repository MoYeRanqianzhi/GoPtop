/**
 * agent-mock-llm.js —— 本地 Anthropic 协议 HTTP 桩（内置 Agent 的「对手模型」替身）。
 *
 * 角色：真实 LLM 端点的**离线替身**——按剧本返回 tool_use 序列，驱动 crates/goptop-agent
 * 的决策循环走完一整局。剧本（参数化，见 `startStub(opts)`）：
 *   开局 chat 一句 → 第一次应手故意落「对方已占点」被拒 → 读错误纠正落子（write 暂存
 *   与 submit 分两轮回，幽灵子窗口可观测）→ 此后有来有回，第 N 手后认输收场；
 *   对手来 chat 即回一句。
 *
 * 契约对齐 crates/goptop-agent/src/llm/anthropic.rs：
 * - 服务端只挂 `POST {base}/messages`（客户端拼 baseUrl + "/messages"）；
 * - 响应 content 块 `tool_use`（input 是对象）+ `text`；`stop_reason` tool_use/end_turn；
 * - 请求侧 `<event>` 注入块在 user 消息的 text 里（agent_loop.event_block），剧本靠它
 *   感知对手落子/聊天/终局——事件带 seq，天然去重键。
 *
 * **纪律**：本文件与全部测试不落任何真实 LLM 端点/key；端点永远是 127.0.0.1 随机口。
 * CORS 头全开（Access-Control-Allow-Origin:*）为阶段⑤ Web 内置模式预留。
 *
 * 独立运行：node agent-mock-llm.js [--port 8099] [--my-color white] [--resign-after 3]
 *                                   [--wait-ms 700] [--stage-ms 1500] [--size 15]
 * 作为模块：const { startStub } = require("./agent-mock-llm.js");
 */
const http = require("http");

/** 睡一拍（剧本的节流/幽灵子窗口都靠它）。 */
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * 剧本引擎：把「最近一轮请求」翻译成下一份 Anthropic 响应。
 * 一个桩实例服务一局（Agent 局单局互斥，剧本不需要多会话路由）。
 */
function createScript(opts) {
  const myColor = opts.myColor || "white";
  // 盘面路数：选点必须跟盘走（围棋 9/13/19 时硬编码 15 会把子落盘外、对局假死）
  const size = opts.size || 15;
  const resignAfter = opts.resignAfter ?? 3; // 我方第 resignAfter 手落定后，下一手认输
  const waitMs = opts.waitMs ?? 700; // 等对手时的应答节流（防预算空转烧穿）
  const stageMs = opts.stageMs ?? 1500; // 暂存与提交之间的间隔=棋盘幽灵子的可观测窗口
  const greeting = opts.greeting ?? "你好！请多指教，这局请慢慢下。";
  const chatReply = opts.chatReply ?? "收到！说得好，我们继续。";
  const farewell = opts.farewell ?? "多谢指教，这局很开心，下次再战！";

  const st = {
    greeted: false,
    seenSeq: new Set(),
    allPoints: new Set(), // "x,y"——双方全部落点（选空点的依据）
    oppLast: null, // 对手最新着法 {x,y}
    oppHandled: null, // 已处理到的对手着法（同一手不回两次）
    myMoves: 0,
    oppMoves: 0,
    chatPending: false, // 对手有未回的 chat
    sentTexts: new Set(), // 我方已发出的 chat 全文（chat 事件只有 from 名字，认文本不认人）
    farewellPending: false,
    farewellSent: false,
    gameOver: false,
    plan: [], // 待发的响应队列（每请求吐一个）
    calls: 0, // 收到的请求数（/__stats 与诊断用）
    idle: 0, // 连续无事件请求数（等待节奏的诊断）
  };

  /** 下一手棋：优先贴着对手刚落的点找空邻点（近战形，对局收敛快），否则向心扫描。 */
  function pickPoint(size) {
    const n = size || 15;
    const c = (n - 1) / 2;
    const free = (x, y) => x >= 0 && y >= 0 && x < n && y < n && !st.allPoints.has(`${x},${y}`);
    if (st.oppLast) {
      const { x, y } = st.oppLast;
      const near = [];
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          if ((dx || dy) && free(x + dx, y + dy)) near.push({ x: x + dx, y: y + dy });
        }
      }
      // 同为空邻点时取更靠中心的（可复现，不掷骰子）
      near.sort((a, b) => (Math.abs(a.x - c) + Math.abs(a.y - c)) - (Math.abs(b.x - c) + Math.abs(b.y - c)));
      if (near.length) return near[0];
    }
    for (let r = 0; r < n; r++) {
      for (let y = 0; y < n; y++) {
        for (let x = 0; x < n; x++) {
          if (Math.max(Math.abs(x - c), Math.abs(y - c)) === r && free(x, y)) return { x, y };
        }
      }
    }
    return null; // 满盘（理论不到：认输远早于满盘）
  }

  /** 从本轮请求里吸收世界状态：事件（seq 去重）与「最后一轮」工具结果（错误识别）。
   *  错误只认**最后一条 user 消息**——历史里的旧错误（如首手的 occupied）永远躺在
   *  上下文里，按全史扫描的话纠正分支会每轮重新触发，把对局拖进无限重下的死循环
   *  （2026-10-08 壳级 e2e 实测：预算被这套空转烧穿，局中途夭折）。 */
  function ingest(body) {
    let lastError = null;
    let farewellAsked = false;
    let events = 0;
    const msgs = body.messages || [];
    let lastResultsIdx = -1;
    for (let i = 0; i < msgs.length; i++) {
      const c = msgs[i].content;
      if (msgs[i].role === "user" && Array.isArray(c) && c.some((b) => b.type === "tool_result")) lastResultsIdx = i;
    }
    for (let i = 0; i < msgs.length; i++) {
      const msg = msgs[i];
      if (msg.role !== "user" || !Array.isArray(msg.content)) continue;
      for (const b of msg.content) {
        if (b.type === "text" && typeof b.text === "string") {
          if (b.text.includes("The game has ended")) farewellAsked = true;
          for (const m of b.text.matchAll(/<event>(\{.*?\})<\/event>/g)) {
            let ev;
            try { ev = JSON.parse(m[1]); } catch { continue; }
            if (typeof ev.seq !== "number" || st.seenSeq.has(ev.seq)) continue;
            st.seenSeq.add(ev.seq);
            events++;
            console.log(`[mock-llm] 事件 seq=${ev.seq} t=${ev.t}${ev.by ? ` by=${ev.by}` : ""}${ev.from ? ` from=${ev.from}` : ""}${ev.t === "move" ? ` @(${ev.x},${ev.y})` : ""}${ev.text ? ` "${String(ev.text).slice(0, 24)}"` : ""}`);
            if (ev.t === "move") {
              const key = `${ev.x},${ev.y}`;
              // 同点重复投递（新 seq 旧坐标）不当作新手：落子只会落在空点
              if (st.allPoints.has(key)) { console.log("[mock-llm]   └ 重复坐标，忽略"); continue; }
              st.allPoints.add(key);
              if (ev.by === myColor) st.myMoves++;
              else { st.oppMoves++; st.oppLast = { x: ev.x, y: ev.y }; }
            } else if (ev.t === "chat") {
              // chat 事件只有 from（名字），协议里拿不到「我是谁」——认文本：自己发过的原样回来
              if (!st.sentTexts.has(ev.text)) st.chatPending = true;
            } else if (ev.t === "game_over") {
              st.gameOver = true;
            }
          }
        } else if (i === lastResultsIdx && b.type === "tool_result" && b.is_error) {
          lastError = typeof b.content === "string" ? b.content : JSON.stringify(b.content);
        }
      }
    }
    if (farewellAsked) st.farewellPending = true;
    if (events === 0) st.idle++; else st.idle = 0;
    if (st.idle > 0 && st.idle % 30 === 0) console.log(`[mock-llm] 连续 ${st.idle} 拍无事件（等待对手中）`);
    return lastError;
  }

  /** 一手棋的两段式计划：write 暂存（立回）→ submit 提交（stageMs 后）——
   *  中间的空窗就是 AgentPage 棋盘幽灵子（agent_status.stagedMove）的观测窗口。 */
  function movePlan(point) {
    return [
      { calls: [{ name: "write", input: { path: "/game/in/move", content: `${point.x},${point.y}` } }] },
      { delay: stageMs, calls: [{ name: "submit", input: { path: "/game/in/move" } }] },
    ];
  }

  const chatBatch = (text) => {
    st.sentTexts.add(text); // chat 事件按文本认领（见 ingest 注）
    return [
      { calls: [{ name: "write", input: { path: "/game/in/chat", content: text } }] },
      { calls: [{ name: "submit", input: { path: "/game/in/chat" } }] },
    ];
  };

  /** 计算下一份响应（st.plan 空时才推进状态机）。 */
  function think(lastError) {
    // 0) 终局收尾：赢家已定——发告别语（循环在 submit 后自行收场）
    if (st.farewellPending && !st.farewellSent) {
      st.farewellSent = true;
      console.log("[mock-llm] 决策：终局告别");
      st.plan = chatBatch(farewell);
      return;
    }
    if (st.gameOver) return; // 收尾已发：文字待机（循环即将 GameOver 退出）

    // 1) 上一轮 submit 被拒：按错误纠正（占点→换空点重写；未轮到→回去等）
    if (lastError) {
      if (/occupied/.test(lastError) && st.oppLast) {
        const p = pickPoint(size);
        console.log(`[mock-llm] 决策：占点被拒，纠正落 ${p ? `(${p.x},${p.y})` : "无点"}`);
        if (p) { st.plan = movePlan(p); return; }
      }
      if (/not your turn/.test(lastError)) return; // 落回等待分支
    }

    // 2) 开局打招呼（chat 两段式，先 write 后 submit）
    if (!st.greeted) {
      st.greeted = true;
      console.log("[mock-llm] 决策：开局打招呼");
      st.plan = chatBatch(greeting);
      return;
    }

    // 3) 对手着法应手
    if (st.oppLast && st.oppLast !== st.oppHandled) {
      st.oppHandled = st.oppLast;
      if (st.myMoves >= resignAfter) {
        // 剧本收场：认输（两段式与普通着子一致，submit 的 terminate 由循环判）
        console.log("[mock-llm] 决策：认输收场");
        st.plan = [
          { calls: [{ name: "write", input: { path: "/game/in/resign", content: "这局我认输，打得愉快！" } }] },
          { calls: [{ name: "submit", input: { path: "/game/in/resign" } }] },
        ];
        return;
      }
      if (st.oppMoves === 1) {
        // 剧本第一手：故意落对手的占点——submit 必被拒，下一轮走纠正分支
        console.log(`[mock-llm] 决策：故意撞占点 (${st.oppLast.x},${st.oppLast.y})`);
        st.plan = [
          { calls: [{ name: "write", input: { path: "/game/in/move", content: `${st.oppLast.x},${st.oppLast.y}` } }] },
          { calls: [{ name: "submit", input: { path: "/game/in/move" } }] },
        ];
        return;
      }
      const p = pickPoint(size);
      console.log(`[mock-llm] 决策：应手落 ${p ? `(${p.x},${p.y})` : "无点"}`);
      if (p) { st.plan = movePlan(p); return; }
    }

    // 4) 对手的 chat 未回
    if (st.chatPending) {
      st.chatPending = false;
      console.log("[mock-llm] 决策：回复聊天");
      st.plan = chatBatch(chatReply);
      return;
    }
    // 5) 无事可做：节流后回纯文本（循环注入 TextOnly 提醒继续转，不断不降）
    return { delay: waitMs, text: "（观察局势中……）" };
  }

  return {
    /** 一次请求 → 一次响应体（Anthropic Messages 形态）。 */
    async handle(body) {
      st.calls++;
      const lastError = ingest(body);
      if (st.plan.length === 0) {
        const direct = think(lastError);
        // think 的「等待」步不走队列（等待不是动作，不该挡住后面的计划推进）：
        // 队列仍空才直发。等待步必须带节流——否则循环 50ms 一轮地把调用预算烧穿。
        if (st.plan.length === 0 && direct) st.plan.push(direct);
      }
      const step = st.plan.length > 0 ? st.plan.shift() : null;
      if (step?.delay) await sleep(step.delay);
      const calls = (step?.calls || []).map((c, i) => ({ ...c, id: `toolu_${st.calls}_${i}` }));
      const text = step?.text ?? (calls.length ? "" : "（继续等待局势。）");
      return {
        content: [
          ...(text ? [{ type: "text", text }] : []),
          ...calls.map((c) => ({ type: "tool_use", id: c.id, name: c.name, input: c.input })),
        ],
        stop_reason: calls.length ? "tool_use" : "end_turn",
      };
    },
    stats() {
      return {
        requests: st.calls,
        oppMoves: st.oppMoves,
        myMoves: st.myMoves,
        gameOver: st.gameOver,
        greeted: st.greeted,
      };
    },
  };
}

/**
 * 起桩。返回 { port, url, script, stats, close }。
 * url 即 llm-config 的 baseUrl（客户端会自己拼 "/messages"）。
 */
function startStub(opts = {}) {
  const script = createScript(opts);
  const server = http.createServer((req, res) => {
    // CORS 全开：阶段⑤ Web 内置模式的 HttpChannel 浏览器通道会跨源打到本桩
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Methods", "POST, GET, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "*");
    if (req.method === "OPTIONS") { res.writeHead(204); res.end(); return; }
    if (req.method === "GET" && (req.url || "").startsWith("/__stats")) {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(script.stats()));
      return;
    }
    if (req.method !== "POST") { res.writeHead(404); res.end(); return; }
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      let body = {};
      try { body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"); } catch { /* 坏体按空处理 */ }
      const names = (body.messages || []).flatMap((m) => (m.content || []))
        .filter((b) => b.type === "tool_use").map((b) => b.name);
      script.handle(body)
        .then((resp) => {
          const tools = resp.content.filter((b) => b.type === "tool_use").map((b) => b.name).join(",");
          console.log(`[mock-llm] #${script.stats().requests} ← ${tools || "text-only"}（入史调用：${names.slice(-3).join(",") || "-"}）`);
          res.writeHead(200, { "Content-Type": "application/json" });
          res.end(JSON.stringify({
            id: `msg_mock_${script.stats().requests}`,
            type: "message",
            role: "assistant",
            model: body.model || "mock-1",
            content: resp.content,
            stop_reason: resp.stop_reason,
            usage: { input_tokens: 40 + (script.stats().requests % 7) * 3, output_tokens: 8 + (script.stats().requests % 5) * 2 },
          }));
        })
        .catch((e) => {
          console.error("[mock-llm] 剧本异常:", e);
          res.writeHead(500, { "Content-Type": "application/json" });
          res.end(JSON.stringify({ type: "error", error: { type: "api_error", message: String(e) } }));
        });
    });
  });
  return new Promise((resolve) => {
    server.listen(opts.port || 0, "127.0.0.1", () => {
      const port = server.address().port;
      resolve({
        port,
        url: `http://127.0.0.1:${port}/v1`,
        script,
        stats: () => script.stats(),
        close: () => new Promise((r) => server.close(r)),
      });
    });
  });
}

module.exports = { startStub, createScript };

/* ---------------- CLI：独立起桩供手工联调 ---------------- */
if (require.main === module) {
  const arg = (name, dflt) => {
    const i = process.argv.indexOf(`--${name}`);
    return i >= 0 ? process.argv[i + 1] : dflt;
  };
  startStub({
    port: Number(arg("port", 8099)),
    myColor: arg("my-color", "white"),
    size: Number(arg("size", 15)),
    resignAfter: Number(arg("resign-after", 3)),
    waitMs: Number(arg("wait-ms", 700)),
    stageMs: Number(arg("stage-ms", 1500)),
  }).then((s) => {
    console.log(`[mock-llm] Anthropic 协议桩就绪：${s.url}/messages（/v1 为 baseUrl）`);
    console.log("[mock-llm] llm-config 示例：protocol=anthropic baseUrl=" + s.url + " model=mock-1 key=stub（任意非空）");
    console.log("[mock-llm] Ctrl-C 退出；GET /__stats 看进度");
  });
}
