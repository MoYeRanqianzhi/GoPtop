//! 围棋 AI —— 自建 MCTS（UCT + RAVE），走子用 go_game_board（libEGo）的模式化 playout。
//!
//! 为什么自建：没有可直接复用的方案。Rapfi/iomrascálaí 是 GPL-3.0（与本项目
//! MIT OR Apache-2.0 不兼容）；KataGo 虽 MIT 但 Eigen 后端仅 10–20 playouts/s，
//! wasm 上更慢；go_game_board 只提供 playout，不含搜索树。
//!
//! playout 速度实测（wasm32 + node）：31,746 playouts/s，平均每手 115 步，
//! 等效约 365 万 moves/s——与 libEGo 自称的 3.6M 吻合，足以支撑搜索。
//!
//! # 三层结构
//! - [`replay`]：core 的 `GameState::history` → `go_game_board::Board`。顺序敏感，
//!   错一步提子/劫判定就全错，`replay_matches_core_board` 单测把坐标约定钉死；
//! - [`Search`]：UCT + RAVE 树搜索。节点只存统计量，局面靠「从根盘按路径重放」恢复；
//! - [`pick_move`]：把访问量最高的着法交回 core 复验（两套规则的差异兜底）。
//!
//! # 时间控制
//! 一律用 `web_time::Instant`，**不是** `std::time::Instant`：wasm32-unknown-unknown
//! 上标准库没有时钟实现，`Instant::now()` 会被编译成 unreachable 陷阱——编译期
//! 不报错，一调用就炸（figrid 正是踩了它才需要 vendor 补丁）。

use std::sync::OnceLock;
use std::time::Duration;

use go_game_board::fast_random::FastRandom;
use go_game_board::{Board, Gammas, MAX_BOARD_SIZE, Player, Sampler, Vertex};
use goptop_core::board::{Coord, Stone};
use goptop_core::game::{GameState, Move};
use web_time::Instant;

use crate::AnalyzeResult;

/// UCT 探索常数。奖励在 `[0, 1]`，`sqrt(2)` 是 UCB1 的理论值，也是 Fuego/Pachi 一档的取值。
const UCT_C: f64 = 1.4142;

/// RAVE 混合权重里的 `b`（Gelly & Silver 2007 的等价参数式
/// `beta = R / (R + n + 4·b²·n·R)`）。
///
/// `b` 决定 RAVE 的半衰期：`4·b²·n·R` 与 `n + R` 同量级时两者等权，b=0.05 对应
/// `n ≈ R ≈ 200`。本项目 1 秒预算下根节点每个子大约几百次访问，正好落在
/// 「前期 RAVE 主导、后期交还 UCT」的区间里。
const RAVE_BIAS: f64 = 0.05;

/// PUCT 的探索常数：`u = C_PUCT · P · √N / (1 + n)`。
///
/// 围棋的根节点有几十上百个候选（9 路约 80、19 路约 360）。纯 UCT 在每点只访问
/// 几十次时分辨不出优劣：实测 9 路空盘前五名的 q 挤在 0.50~0.53（标准误约 0.018），
/// `argmax` 等于在噪声里取最大值——同一随机种子下首选会随搜索量在 (6,2)/(1,5)/
/// (5,2)/(4,4)/(2,4)/(1,2) 之间漂移，其中好几个在二线。先验让预算先落到模式化
/// 策略认为像样的点上，这是本实现相对纯 UCT 的唯一改动。
const C_PUCT: f64 = 1.6;

/// 估计根节点先验时的采样次数。3000 次约占 1 秒预算的 3%（实测单次采样约 10μs），
/// 换来的是一棵不再从噪声起步的树。
const PRIOR_SAMPLES: u32 = 3000;

/// 两次 deadline 检查之间至少做多少次 playout。
///
/// 单次 playout 约 30μs，128 次约 4ms——超时最多多花 4ms，相对千毫秒预算是噪声；
/// 而每次都读时钟会把 `performance.now()` 的开销摊进 wasm 的热路径。
const DEADLINE_CHECK_EVERY: u64 = 128;

/// 单次 playout 的着法数上限，纯粹是死循环保险。
///
/// `sample_move` 只在模式化 gamma 全为 0 时返回 pass，理论上必然收敛（上游 benchmark
/// 就是无保险地跑这个循环，实测 9 路平均 115 手）。但 wasm 单线程下死循环会让整个
/// Worker 永久卡死、界面上表现为「AI 思考中」永不消失，而循环里多一个计数器几乎不要钱。
const MAX_PLAYOUT_MOVES: usize = 4096;

/// `FastRandom` 是 Park-Miller 最小标准发生器，种子需小于 2^31-1（否则序列退化）。
/// 固定种子：同一个局面重放两次得到同一棵树，测试才敢断言胜率。
const RNG_SEED: u32 = 12345;

/// 模式化 playout 的 gamma 表：`Gammas::new()` 要建 2^20 × 2 个 f64（约 16MB）
/// 并逐项判合法性，是全部固定开销里最贵的一项。
///
/// 与 `gomoku` 的 NNUE 权重同样用 `OnceLock` 常驻：一场对局里 analyze 会被调上百次，
/// 每次重建 16MB 既慢又会把 wasm 的线性内存翻倍；wasm 单线程，无并发问题。
static GAMMAS: OnceLock<Gammas> = OnceLock::new();

fn gammas() -> &'static Gammas {
    GAMMAS.get_or_init(Gammas::new)
}

/// `(行棋方, 顶点)` → AMAF 位图下标。颜色必须参与编码：同一顶点黑白都会落，
/// 混在一起会把对手的着法算成自己的 RAVE 样本。
fn move_id(player: Player, v: Vertex) -> u32 {
    (player as usize * Vertex::COUNT + usize::from(v)) as u32
}

/// 把 core 的 `history` 按落子顺序重放到 go_game_board 的棋盘上。
///
/// **顺序不能乱**：提子与劫都由历史决定，打乱顺序会让某些手在自己的规则下变成
/// 自杀或劫争回提，整盘棋的提子数就对不上了。
///
/// 坐标约定：core 是 `Coord{x, y}` 且棋盘语义 `board[y][x]`，go_game_board 是
/// `Vertex::from_coords(row, col)`——即 `row = y`、`col = x`。写反会得到转置的棋盘，
/// 而且**不会报错**（19 路以下转置仍是合法局面），只能靠单测锁住。
///
/// 个别手在 go_game_board 下会非法：两者是各自独立实现的规则（core 用洪泛法逐点算气、
/// go_game_board 用邻接计数器 + 链表），边界情形（多链提子、自杀的最后一气）并不保证
/// 逐点一致。这里跳过该手继续重放——漏一手的局部偏差，远小于「整局作废、AI 对着空盘
/// 搜索」的错法。
fn replay(board: &mut Board, state: &GameState) {
    let mut player = Player::Black;
    for mv in &state.history {
        match mv {
            Move::Place(c) => {
                // 核心层的物理盘恒为 19×19（与 go_game_board 的 MAX_BOARD_SIZE 同值），
                // 但反序列化来的状态可能带越界坐标，而 `Vertex::from_coords` 越界会
                // panic 成 wasm 的 unreachable 陷阱。1 条比较换掉一个崩溃点。
                if (c.x as usize) < MAX_BOARD_SIZE && (c.y as usize) < MAX_BOARD_SIZE {
                    let v = Vertex::from_coords(c.y as isize, c.x as isize);
                    // 9/13 路的逻辑边界由 `with_size` 天然生效：逻辑盘外全是 OffBoard，
                    // `is_legal` 直接返回 false，不必再单独判一次 kind.size()。
                    if board.is_legal(player, v) {
                        board.play_legal(player, v);
                    }
                }
                player = player.opponent();
            }
            Move::Pass => {
                board.play_legal(player, Vertex::pass());
                player = player.opponent();
            }
            // 认输不换手（core 同此语义）；且认输必然已置 winner，
            // lib.rs 的 `analyze` 会在进搜索前就返回确定结果，这里到不了。
            Move::Resign => {}
        }
    }
}

/// MCTS 树节点。
///
/// 不持有 `Board` 副本：一次 1 秒搜索会展开上万个节点，而一个 `Board` 有十余张
/// 363 项的 `VertexMap`（约 25KB），挂进树里就是数百 MB 的内存与拷贝。局面统一由
/// 「从根盘按路径重放」在函数局部的工作盘上恢复，见 [`Search::iterate`]。
struct Node {
    /// 进入本节点的着法。根节点无意义（恒为 pass），RAVE 更新会跳过根。
    mv: Vertex,
    /// 本节点轮到谁走。**谁下了本手是 `player.opponent()`**——胜负回传按它判方向，
    /// 弄反会让整棵树的胜率取镜像（胜率约等于 1 减原值，且不会报错）。
    player: Player,
    /// 树中深度，根为 0。
    depth: u32,
    /// 已展开的子节点（按展开先后，非按访问量）。
    children: Vec<u32>,
    /// 尚未展开的合法着法（含 pass）。展开顺序随机，pass 排在首位——`iterate` 从
    /// 末位取，所以它最后一个才被展开。
    untried: Vec<Vertex>,
    /// 真实访问次数（UCT 项的分母）。
    visits: u32,
    /// 真实胜数，从 `player.opponent()`（本手的落子方）视角计。
    wins: f64,
    /// AMAF 访问次数（RAVE 项的分母）。
    rave_visits: u32,
    /// AMAF 胜数，视角同 `wins`。
    rave_wins: f64,
    /// PUCT 的先验概率 P(s,a)。根节点的子节点用模式化策略的采样频率（已归一化）；
    /// 更深层用均匀值——那里每个节点只有几十次访问，先验的边际收益抵不上再采一轮。
    prior: f64,
}

/// 一次搜索的全部可变态。
///
/// `root` 与 `work` 的分工：`root` 是根局面，只读；`work` 是工作盘，每次迭代先把
/// `root` 拷回来再沿选择路径逐手推进，最后直接从它起跑 playout。go_game_board 不提供
/// undo（提子与链表的回滚成本比拷贝高），所以「复位」就是一次 `load`——约 25KB 的
/// memcpy，相对 30μs 的 playout 是可忽略的零头。
struct Search<'a> {
    nodes: Vec<Node>,
    gammas: &'a Gammas,
    sampler: Sampler,
    random: FastRandom,
    root: Board,
    work: Board,
    /// 本次迭代从根到叶的节点索引，回传时沿它更新。
    path: Vec<u32>,
    /// AMAF 位图（下标见 [`move_id`]），标记本次 playout 里出现过的着法。
    amaf: Vec<bool>,
    /// 本次 playout 实际落下的着法，用于下一次复位 `amaf`（按需清，比 fill 全表快）。
    played: Vec<u32>,
    /// 树已展开的最大深度，直接作为 `AnalyzeResult::depth`。
    max_depth: u32,
    /// 根节点各着法的先验概率，按**顶点下标**索引（`Vertex` 可直接转 usize）。
    priors: Vec<f64>,
}

impl Search<'_> {
    /// 跑一次完整的 MCTS 迭代：选择 → 扩展 → 模拟 → 回传。
    fn iterate(&mut self) {
        self.path.clear();
        self.work.load(&self.root);
        let mut cur: u32 = 0;
        self.path.push(0);

        loop {
            // 有未试着手就扩展（取末位，pass 固定最后展开，见 legal_moves）。
            if let Some(mv) = self.nodes[cur as usize].untried.pop() {
                let mover = self.nodes[cur as usize].player;
                self.work.play_legal(mover, mv);
                let child = self.expand(cur, mv, mover);
                self.path.push(child);
                break;
            }
            // 无未试着手：按 UCT+RAVE 继续下潜；连子节点都没有说明是终局叶子。
            let Some(child) = self.select(cur) else { break };
            let (mv, mover) = {
                let n = &self.nodes[child as usize];
                (n.mv, n.player.opponent())
            };
            self.work.play_legal(mover, mv);
            self.path.push(child);
            cur = child;
        }

        let winner = self.playout();
        self.backprop(winner);
    }

    /// 在当前工作盘上生成 `player` 的全部合法着法。
    ///
    /// 只遍历空点而非整盘：合法着法必然落在空点上，而空点列表本身已经排除了
    /// 9/13 路的逻辑盘外区域（`with_size` 把它们标成了 OffBoard，从不入列）。
    fn legal_moves(&mut self, player: Player) -> Vec<Vertex> {
        let n = self.work.empty_vertex_count();
        let mut moves: Vec<Vertex> = Vec::with_capacity(n + 1);
        for i in 0..n {
            let v = self.work.empty_vertex(i);
            if self.work.is_legal(player, v) {
                moves.push(v);
            }
        }

        // Fisher-Yates 打乱展开顺序。固定扫描序（恒从左上角开始）会让短预算下的
        // best_move 由棋盘序而不是胜率决定——9 路空盘测试正是防这个。
        for i in (1..moves.len()).rev() {
            let j = (self.random.get_next_uint() as usize) % (i + 1);
            moves.swap(i, j);
        }

        // pass 放在**首位**：`iterate` 用 `pop()` 从末位取未试着手，首位即最后一个
        // 才展开。空盘/中盘时 pass 的胜率估计是纯噪声，先展开会让它白占一次
        // 「未访问优先」的名额，短预算下甚至直接被选成访问量最高的子节点。
        moves.insert(0, Vertex::pass());
        moves
    }

    /// 新建子节点。调用前 `self.work` 必须已经落下 `mv`——未试着手集是**落子之后**
    /// 的局面上的合法着法。
    fn expand(&mut self, parent: u32, mv: Vertex, mover: Player) -> u32 {
        let depth = self.nodes[parent as usize].depth + 1;
        let untried = self.legal_moves(mover.opponent());
        // 根的子节点用模式化先验；更深层用按候选数归一化的均匀值——那里每个节点
        // 只有几十次访问，先验的边际收益抵不上为每个节点再采一轮的代价。
        let prior = if parent == 0 {
            self.priors[usize::from(mv)]
        } else {
            let k = untried.len() as f64;
            if k > 0.0 { 1.0 / k } else { 1.0 }
        };
        if depth > self.max_depth {
            self.max_depth = depth;
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            mv,
            player: mover.opponent(),
            depth,
            children: Vec::new(),
            untried,
            visits: 0,
            wins: 0.0,
            rave_visits: 0,
            rave_wins: 0.0,
            prior,
        });
        self.nodes[parent as usize].children.push(id);
        id
    }

    /// 在 `parent` 的子节点里选一个下潜。返回 `None` 表示无子可下（终局）。
    fn select(&self, parent: u32) -> Option<u32> {
        let p = &self.nodes[parent as usize];
        if p.children.is_empty() {
            return None;
        }
        // 有子节点的父节点必然访问过（展开那一轮的回传会 +1），max(1) 只为杜绝 ln(0)。
        let sqrt_parent = (p.visits.max(1) as f64).sqrt();
        let mut best = None;
        let mut best_score = f64::NEG_INFINITY;

        for &c in &p.children {
            let n = &self.nodes[c as usize];
            let v = n.visits as f64;
            // PUCT：先验决定「先试哪些」，探索项决定「何时回头」。未访问时 q 记 0，
            // 由 u 项按先验排序——纯 UCT 把所有未访问点一律记 +inf，等于按展开顺序
            // （随机的）挑第一个，那正是开局首选在噪声里漂移的来源。
            let exploit = if n.visits == 0 { 0.0 } else { self.blended_q(n) };
            let u = C_PUCT * n.prior * sqrt_parent / (1.0 + v);
            let score = exploit + u;
            if score > best_score {
                best_score = score;
                best = Some(c);
            }
        }
        best
    }

    /// `(1-β)·W/n + β·W_rave/R`：真实胜率与 RAVE 胜率的 β 加权混合。
    fn blended_q(&self, n: &Node) -> f64 {
        let v = n.visits as f64;
        let q = n.wins / v;
        if n.rave_visits == 0 {
            return q;
        }
        let r = n.rave_visits as f64;
        let beta = r / (r + v + 4.0 * RAVE_BIAS * RAVE_BIAS * v * r);
        (1.0 - beta) * q + beta * (n.rave_wins / r)
    }

    /// 从当前工作盘跑一局随机 playout，返回胜方。
    fn playout(&mut self) -> Player {
        for &m in &self.played {
            self.amaf[m as usize] = false;
        }
        self.played.clear();

        self.sampler.new_playout(&self.work, self.gammas);
        let mut steps = 0usize;
        while !self.work.both_player_pass() && steps < MAX_PLAYOUT_MOVES {
            steps += 1;
            let pl = self.work.act_player();
            let v = self.sampler.sample_move(&self.work, &mut self.random);
            self.work.play_legal(pl, v);
            self.sampler.move_played(&self.work, self.gammas);
            let id = move_id(pl, v);
            self.played.push(id);
            self.amaf[id as usize] = true;
        }
        self.work.playout_winner()
    }

    /// 沿本次迭代的路径回传胜负。
    ///
    /// RAVE 的判据是「本节点的着法是否出现在这局 playout 的着法表里」（AMAF），
    /// 这正是标准实现给「父局面 + 该着法」这一对统计量用的判据。
    ///
    /// 与标准实现的差别在**更新面**：这里只更新路径上的节点，不更新路径上每个节点的
    /// 全部候选着法（Fuego / Pachi 是在每个路径节点上遍历它的**子节点**、拿子节点的
    /// 着法去查 playout 表）。省掉的是「每个节点挂一张全覆盖着法统计表」的内存，
    /// 代价落在样本量上，而且不是「慢半拍」那么轻：
    ///
    /// 路径节点的着法在进入 playout **之前**就已经落在工作盘上了（下降时逐手 `play_legal`
    /// 下去），所以 `amaf[mid]` 要成立，只能等这一点在 playout 里被提掉、再被同一方下回来。
    /// 样本集因此是「该着法被提后又下回的那些 playout」，而不是「所有经过该节点的 playout」。
    /// 实测（9 路、4 子局面、6000 次迭代）：
    ///
    /// ```text
    /// 真实访问 18506 / RAVE 访问 1889（约 10%；分深度看 depth1/depth2 都是 15%）
    /// beta：有 RAVE 样本的节点均值 0.165，根的子节点（按访问量加权）0.110
    /// q = 0.337 vs rave_q = 0.277 —— AMAF 估计系统性更悲观
    /// ```
    ///
    /// 结论：RAVE 不是「恒为 0 的死字段」，确实在起作用——把 `RAVE_BIAS` 抬到 1e6 让
    /// beta→0，9 路中盘子局（8000 次迭代）的根节点首选从 (6,4) 变成 (6,3)、根胜率
    /// 从 0.5006 变成 0.5125。但样本稀、且带「先被提掉」这个选择性偏差，所以
    /// `blended_q` 里 RAVE 那一半的可信度低于教科书版本。
    fn backprop(&mut self, winner: Player) {
        for i in 0..self.path.len() {
            let id = self.path[i] as usize;
            let n = &mut self.nodes[id];
            // 根节点也要计数（它是子节点探索项里的 `ln(parent_visits)`），但它不代表
            // 任何一手：既没有落子方，也没有可更新的 RAVE 样本。
            n.visits += 1;
            if id == 0 {
                continue;
            }
            // 本手的落子方。胜负与之相同才算本节点赢。
            let mover = n.player.opponent();
            if winner == mover {
                n.wins += 1.0;
            }
            let mid = move_id(mover, n.mv) as usize;
            if self.amaf[mid] {
                n.rave_visits += 1;
                if winner == mover {
                    n.rave_wins += 1.0;
                }
            }
        }
    }
}

/// 用模式化策略估计根节点各候选着法的先验概率（PUCT 的 P）。
///
/// 做法是让 `Sampler` 在根局面上按 gamma 分布采样**开局着法**、统计频率——这正是
/// 「模式化策略认为这一手有多像样」的直接度量，不必另接一套模式表。
///
/// 拉普拉斯平滑（每候选 +1）不能省：gamma 从不选的点也要留一条缝，否则它的 u 项
/// 恒为 0、永远竞争不过别人，等于把那些点从搜索空间里删掉了。
fn root_priors(root: &Board, gammas: &Gammas, cands: &[Vertex]) -> Vec<f64> {
    let mut priors = vec![0.0f64; Vertex::COUNT];
    if cands.is_empty() {
        return priors;
    }
    let mut counts = vec![0u32; cands.len()];
    let mut sampler = Sampler::new(root, gammas);
    let mut random = FastRandom::new(0x5eed_1234);
    for _ in 0..PRIOR_SAMPLES {
        // 每次采样前复位到根局面：要的是「开局第一手」的分布，不是整局的着法分布
        sampler.new_playout(root, gammas);
        let v = sampler.sample_move(root, &mut random);
        if let Some(i) = cands.iter().position(|&c| c == v) {
            counts[i] += 1;
        }
    }
    let smoothed: Vec<f64> = counts.iter().map(|&c| f64::from(c) + 1.0).collect();
    let total: f64 = smoothed.iter().sum();
    for (i, &c) in cands.iter().enumerate() {
        priors[usize::from(c)] = smoothed[i] / total;
    }
    priors
}

/// 从根的子节点里挑着法，并过一遍 core 的合法性。
///
/// 这是**静默失败陷阱**的兜底：搜索内部按 go_game_board 的规则走，而规则真源是 core，
/// 两套独立实现的判定在边界情形（多链提子、自杀的最后一气）不保证逐点一致。一旦分叉，
/// 前端只会看到「AI 不动了」，看不出是规则差异。所以候选按访问量从高到低逐个交给
/// `GameState::try_play` 试下（在克隆上试，不动真实对局），全非法才返回 `None`。
///
/// 停一手（pass）不在此列，且被显式跳过：`AnalyzeResult::best_move` 是
/// `Option<(u8, u8)>`，表达不了 pass；返回 `None` 又会被前端当成「AI 无处可下」
/// 弹错并让对局停摆。pass 留在搜索里（它影响胜率估计与收官判断），只是不作为着法
/// 返回——真的只剩 pass 可走时退化成「继续找最大的一手」，比卡死好。
fn pick_move(state: &GameState, search: &Search<'_>) -> Option<(u8, u8)> {
    let mut kids = search.nodes[0].children.clone();
    kids.sort_by_key(|&c| std::cmp::Reverse(search.nodes[c as usize].visits));

    for c in kids {
        let v = search.nodes[c as usize].mv;
        if v == Vertex::pass() {
            continue;
        }
        let (x, y) = (v.column() as u8, v.row() as u8);
        let mut probe = state.clone();
        if probe.try_play(Move::Place(Coord::new(x, y))).is_ok() {
            return Some((x, y));
        }
    }
    None
}

pub fn analyze(
    state: &GameState,
    size: u8,
    my_color: Stone,
    budget_ms: u32,
    want_move: bool,
) -> AnalyzeResult {
    // gamma 表必须在**取时钟之前**备好：它是进程级一次性的构造开销（实测 debug 下
    // 约半秒、远大于整个思考预算），算进预算会让第一次 analyze 只跑出个位数的 playout
    // ——表现出来就是「开局第一手 AI 乱下」，而且只在第一次调用时出现，极难复现。
    // `elapsed_ms` 因此不含这笔一次性开销（与 gomoku 的 NNUE 权重同一口径）。
    let gammas = gammas();

    // 根局面。`with_size` 只把逻辑盘内标成 Empty，其余（含 9/13 路的盘外）是
    // OffBoard——上面 `legal_moves` / `replay` 都靠这一点天然处理逻辑边界。
    let mut root = Board::with_size(size as usize, size as usize);
    replay(&mut root, state);
    let root_player = root.act_player();

    let sampler = Sampler::new(&root, gammas);
    let start = Instant::now();
    let mut search = Search {
        nodes: vec![Node {
            mv: Vertex::pass(),
            player: root_player,
            depth: 0,
            children: Vec::new(),
            untried: Vec::new(),
            visits: 0,
            wins: 0.0,
            rave_visits: 0,
            rave_wins: 0.0,
            // 根节点没有父节点，先验用不上（它不参与任何 select）
            prior: 0.0,
        }],
        gammas,
        sampler,
        random: FastRandom::new(RNG_SEED),
        work: root.clone(),
        root,
        path: Vec::with_capacity(64),
        amaf: vec![false; 2 * Vertex::COUNT],
        played: Vec::with_capacity(512),
        max_depth: 0,
        priors: vec![0.0; Vertex::COUNT],
    };
    let root_untried = search.legal_moves(root_player);
    // 先验要用根局面与根候选算，所以得等 legal_moves 出来之后再填
    let priors = root_priors(&search.root, gammas, &root_untried);
    search.priors = priors;
    search.nodes[0].untried = root_untried;

    let budget = Duration::from_millis(u64::from(budget_ms));
    let mut playouts = 0u64;
    let mut next_check = 1u64;
    loop {
        search.iterate();
        playouts += 1;
        if playouts >= next_check {
            if start.elapsed() >= budget {
                break;
            }
            next_check = playouts + DEADLINE_CHECK_EVERY;
        }
    }

    // 胜率取根节点全部子节点的加权平均：每个子节点的 wins/visits 统计的都是
    // 「根节点行棋方取胜」的频率，而根节点自己的 wins 没有意义（它不代表任何一手）。
    let mut wins = 0.0f64;
    let mut visits = 0u64;
    for &c in &search.nodes[0].children {
        let n = &search.nodes[c as usize];
        wins += n.wins;
        visits += u64::from(n.visits);
    }
    let p_root_wins = if visits == 0 { 0.5 } else { wins / visits as f64 };
    let p_black = if root_player == Player::Black { p_root_wins } else { 1.0 - p_root_wins };
    let win_rate = match my_color {
        Stone::White => 1.0 - p_black,
        _ => p_black,
    }
    .clamp(0.0, 1.0);

    let best_move = if want_move { pick_move(state, &search) } else { None };

    AnalyzeResult {
        best_move,
        win_rate,
        depth: search.max_depth,
        nodes: playouts,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Nat` 带来 `Vertex::all()`，`amaf_move_id_keeps_colors_apart` 要遍历全部顶点。
    use go_game_board::{Color, Nat};
    use goptop_core::game::GameKind;

    fn go(size: u8) -> GameState {
        GameState::new(GameKind::Go { size })
    }

    /// 按顺序执行一手，失败即测试失败。
    fn play(s: &mut GameState, mv: Move) {
        assert!(s.try_play(mv.clone()).is_ok(), "非法着法 {mv:?}");
    }

    fn place(s: &mut GameState, x: u8, y: u8) {
        play(s, Move::Place(Coord::new(x, y)));
    }

    fn to_stone(player: Player) -> Stone {
        match player {
            Player::Black => Stone::Black,
            Player::White => Stone::White,
        }
    }

    /// 坐标约定的锁定测试：core 的 `board[y][x]` 与 `Vertex::from_coords(row, col)`
    /// 之间的搬运一旦写反，19 路以下会得到一个**转置的合法局面**——不报错，只是
    /// AI 一直在错的盘上搜索。这里逐点比对，并顺带验证轮手一致。
    #[test]
    fn replay_matches_core_board() {
        let mut s = go(9);
        for &(x, y) in &[(2u8, 3u8), (3, 3), (3, 2), (4, 3), (4, 2), (5, 5)] {
            place(&mut s, x, y);
        }
        let mut b = Board::with_size(9, 9);
        replay(&mut b, &s);

        for y in 0..9isize {
            for x in 0..9isize {
                let want = s.board.get(Coord::new(x as u8, y as u8)).unwrap();
                let got = match b.color_at(Vertex::from_coords(y, x)) {
                    Color::Black => Stone::Black,
                    Color::White => Stone::White,
                    _ => Stone::Empty,
                };
                assert_eq!(got, want, "({x},{y}) 搬运错位");
            }
        }
        assert_eq!(to_stone(b.act_player()), s.to_move);
    }

    /// 坐标搬运的**独立**复核：硬编码顶点下标，不复用 `from_coords` / `column()` /
    /// `row()` 这一整套约定。
    ///
    /// 上面那条 `replay_matches_core_board` 与实现共用同一份坐标约定，约定本身整体
    /// 写反是看不出来的；这条把下标算术写死，约定一旦被动过就会立刻变红。
    ///
    /// 下标公式：`index = (row + 1) * ROW_SIZE + (col + 1)`，`ROW_SIZE = 19 + 2 = 21`
    /// （四周各一圈哨兵，见 go_game_types 的 `Vertex::from_coords`）。
    #[test]
    fn coords_are_not_transposed_raw_index() {
        // 19 路，两个刻意不对称的坐标：先手黑 (x=2,y=5)、后手白 (x=11,y=3)。
        let mut s = go(19);
        place(&mut s, 2, 5);
        place(&mut s, 11, 3);
        let mut b = Board::with_size(19, 19);
        replay(&mut b, &s);

        assert_eq!(b.color_at(Vertex::from(129)), Color::Black, "row5/col2 不是黑（下标 129）");
        assert_eq!(b.color_at(Vertex::from(96)), Color::White, "row3/col11 不是白（下标 96）");
        // 转置后的落点必须为空，写反 (x,y) 时会落在这里。
        assert_eq!(b.color_at(Vertex::from(69)), Color::Empty, "下标 69 被转置污染");
        assert_eq!(b.color_at(Vertex::from(256)), Color::Empty, "下标 256 被转置污染");
        // 黑一手、白一手之后仍该黑走。
        assert_eq!(b.act_player(), Player::Black, "两手后该轮黑");
    }

    /// 提子方向的独立复核：被提掉的那颗子必须是转置后**另一个**下标。
    ///
    /// 只比对落子不足以钉住方向——提子还要走一遍邻接关系，坐标写反时提掉的会是
    /// 另一个点。这里直接断言哪个下标变空、哪些下标是黑。
    #[test]
    fn capture_removes_the_right_vertex() {
        // 黑 (1,5)(3,5)(2,4)(2,6) 围死白 (2,5)，白在 (0,0)(0,1) 垫手。
        let mut s = go(19);
        place(&mut s, 1, 5);
        place(&mut s, 2, 5);
        place(&mut s, 3, 5);
        place(&mut s, 0, 0);
        place(&mut s, 2, 4);
        place(&mut s, 0, 1);
        place(&mut s, 2, 6);
        assert_eq!(s.board.get(Coord::new(2, 5)), Some(Stone::Empty), "前提局面不成立：白没被提");

        let mut b = Board::with_size(19, 19);
        replay(&mut b, &s);
        // row5/col2 = 129 必须被提空；四邻 108(row4,col2) / 130(row5,col3) /
        // 150(row6,col2) 是黑；转置点 69(row2,col5) 必须没被动过。
        assert_eq!(b.color_at(Vertex::from(129)), Color::Empty, "下标 129 没被提");
        assert_eq!(b.color_at(Vertex::from(108)), Color::Black, "下标 108 应是黑");
        assert_eq!(b.color_at(Vertex::from(130)), Color::Black, "下标 130 应是黑");
        assert_eq!(b.color_at(Vertex::from(150)), Color::Black, "下标 150 应是黑");
        assert_eq!(b.color_at(Vertex::from(69)), Color::Empty, "转置点 69 被误写");
    }

    /// `move_id` 的颜色编码：黑白必须落在互不重叠的下标区间。
    ///
    /// 编码里丢掉颜色也能编译、也能跑，但会把对手的着法算成自己的 RAVE 样本；
    /// 树本身不会报错，只是胜率估计悄悄变坏——只能靠这条钉住。
    #[test]
    fn amaf_move_id_keeps_colors_apart() {
        // 不变量一：黑白各占 `Vertex::COUNT` 宽的一半，黑在下半、白在上半。
        // （`Vertex::COUNT` 是 19×19 含哨兵的顶点数 + 2 个哨兵值，= 443；`pass` 是
        // 441、`none` 是 442，所以 443 这个数本身不出现。）
        for v in Vertex::all() {
            let b = move_id(Player::Black, v) as usize;
            let w = move_id(Player::White, v) as usize;
            assert!(b < Vertex::COUNT, "黑 {v:?} → {b} 越出下半区");
            assert!(
                (Vertex::COUNT..2 * Vertex::COUNT).contains(&w),
                "白 {v:?} → {w} 越出上半区"
            );
        }
        // 不变量二：`amaf` 位图的长度刚好装得下全部 (颜色, 顶点) 组合，且两两不撞。
        let mut seen = vec![false; 2 * Vertex::COUNT];
        for pl in [Player::Black, Player::White] {
            for v in Vertex::all() {
                let id = move_id(pl, v) as usize;
                assert!(!seen[id], "({pl:?}, {v:?}) 的下标 {id} 与别的组合撞车");
                seen[id] = true;
            }
        }
        assert_eq!(seen.len(), 2 * Vertex::COUNT);
    }

    /// 合法性兜底必须真的接在返回路径上。
    ///
    /// 平时的局面里 core 与 libEGo 的判定一致（实测 39543 个点零分歧），所以自然对局
    /// 触发不到兜底；这条手工往根节点的子节点里塞一个 core 必拒的候选（已在盘上的点），
    /// 直接逼兜底生效。删掉 `pick_move` 里的 `try_play` 复验，这条立刻变红。
    #[test]
    fn pick_move_rejects_moves_core_would_refuse() {
        let mut s = go(9);
        place(&mut s, 4, 4);
        place(&mut s, 2, 2);

        let g = gammas();
        let mut root = Board::with_size(9, 9);
        replay(&mut root, &s);
        let root_player = root.act_player();
        let mut search = Search {
            nodes: vec![Node {
                mv: Vertex::pass(),
                player: root_player,
                depth: 0,
                children: Vec::new(),
                untried: Vec::new(),
                visits: 0,
                wins: 0.0,
                rave_visits: 0,
                rave_wins: 0.0,
                prior: 0.0,
            }],
            gammas: g,
            sampler: Sampler::new(&root, g),
            random: FastRandom::new(RNG_SEED),
            work: root.clone(),
            root,
            path: Vec::with_capacity(64),
            amaf: vec![false; 2 * Vertex::COUNT],
            played: Vec::with_capacity(512),
            max_depth: 0,
            priors: vec![0.0; Vertex::COUNT],
        };
        let untried = search.legal_moves(root_player);
        let priors = root_priors(&search.root, g, &untried);
        search.priors = priors;
        search.nodes[0].untried = untried;
        search.iterate();
        assert!(!search.nodes[0].children.is_empty(), "根节点没展开出子节点");

        // 已在盘上的 (4,4)：core 必拒（Occupied）。给它最高的访问量，逼它排在最前面。
        let occupied = Vertex::from_coords(4, 4);
        let id = search.nodes.len() as u32;
        search.nodes.push(Node {
            mv: occupied,
            player: Player::White,
            depth: 1,
            children: Vec::new(),
            untried: Vec::new(),
            visits: 9_999,
            wins: 9_999.0,
            rave_visits: 0,
            rave_wins: 0.0,
            // 先验给足：这条测试要的是「访问量最高」的候选排在最前，先验不该干扰
            prior: 1.0,
        });
        search.nodes[0].children.push(id);

        let got = pick_move(&s, &search).expect("兜底把整个根节点都放弃了");
        assert_ne!(got, (4, 4), "兜底没生效：返回了 core 拒绝的着法");
        let mut probe = s.clone();
        assert!(probe.try_play(Move::Place(Coord::new(got.0, got.1))).is_ok());

        // 所有候选都非法时必须返回 None，而不是把非法着法交出去。
        let kids = search.nodes[0].children.clone();
        for c in kids {
            search.nodes[c as usize].mv = occupied;
        }
        assert_eq!(pick_move(&s, &search), None, "全部非法时应返回 None");
    }

    /// 胜率必须真的随局面变，而不是恒 0.5。
    ///
    /// `win_rate_viewpoint_is_complementary` 只断言两次视角之和 ≈ 1——恒 0.5 也能过。
    /// 这条换一个**决定性的**局面（黑白吃 16 子），要求两个视角给出方向明确且互补的值。
    #[test]
    fn win_rate_reflects_the_position() {
        // 预算压到 150ms：局面是「提了稳赢、不提稳输」，几百次 playout 就足以分辨，
        // 而整套测试是并行跑的，没必要再多占 CPU。
        let s = atari_position(false);
        let b = analyze(&s, 9, Stone::Black, test_budget(150), false);
        let w = analyze(&s, 9, Stone::White, test_budget(150), false);
        assert!(b.win_rate > 0.7, "黑大优却只有 {}", b.win_rate);
        assert!(w.win_rate < 0.3, "白大劣却有 {}", w.win_rate);
        assert!(
            (b.win_rate + w.win_rate - 1.0).abs() < 0.05,
            "视角不互补：黑 {} 白 {}",
            b.win_rate,
            w.win_rate
        );
    }

    /// 9/13 路的逻辑边界：`with_size` 之外的坐标必须是 OffBoard，`is_legal` 一律拒绝。
    #[test]
    fn logical_bounds_are_off_board() {
        let mut s = go(9);
        place(&mut s, 4, 4);
        let mut b = Board::with_size(9, 9);
        replay(&mut b, &s);

        // (9,0) 在 9 路里是逻辑盘外，但在 19 路的物理存储里是合法索引。
        let v = Vertex::from_coords(9, 0);
        assert_eq!(b.color_at(v), Color::OffBoard);
        assert!(!b.is_legal(Player::White, v));
        // 空点表里也不该出现它。
        let mut seen = false;
        for i in 0..b.empty_vertex_count() {
            if b.empty_vertex(i) == v {
                seen = true;
            }
        }
        assert!(!seen, "逻辑盘外顶点混进了空点表");
    }

    /// 9 路空盘：给出合法着法、不 panic、胜率贴 0.5（空盘本就没有优势方），
    /// 且在预算内跑够了 playout。
    ///
    /// **这里刻意不断言首选方位。** 实测（release、固定迭代次数、同一随种子）空盘根节点
    /// 各候选的 q 全部挤在 0.50~0.53：
    ///
    /// ```text
    /// iters=8000  top5=(4,4)v159q0.57 (1,2)v150q0.58 (3,2)v148q0.54 (1,5)v143q0.52 (5,2)v136q0.52
    /// iters=32000 top5=(4,3)v732q0.53 (2,4)v697q0.52 (3,2)v612q0.50 (3,4)v570q0.50 (5,2)v569q0.50
    /// ```
    ///
    /// v=732 时 q 的标准误约 0.018，0.53 与 0.50 只差 1.6σ；在 80 个候选里取最大值，
    /// 单靠噪声就能冒出 +0.03。**随机 playout 分辨不出空盘点位优劣**，`argmax(visits)`
    /// 于是等价于在噪声里取最大值——首选会随 playout 数漂移（500→(6,2)、1000→(1,5)、
    /// 8000→(4,4)、16000→(2,4)），其中 (1,5)/(1,2)/(3,1) 都在二线上。
    ///
    /// 原先断言「落在中心 5×5」因此是**随机变红**的：12000ms（debug 下的
    /// `test_budget(1000)`）预算跑出 11777 次 playout 时首选是 (3,1)，测试就挂。
    /// 该断言测的是「playout 数恰好落在哪个区间」，不是引擎行为，已改为只断言真正
    /// 成立的性质。首选漂移已由 PUCT 先验修掉，回归守卫见
    /// `go9_empty_first_move_is_stable`。
    #[test]
    fn go9_empty_returns_a_sane_result() {
        let s = go(9);
        let r = analyze(&s, 9, Stone::Black, test_budget(300), true);
        let (x, y) = r.best_move.expect("空盘必须有可下之处");
        let mut probe = s.clone();
        assert!(
            probe.try_play(Move::Place(Coord::new(x, y))).is_ok(),
            "空盘首选 ({x},{y}) 被 core 拒绝"
        );
        assert!(
            (0.35..=0.65).contains(&r.win_rate),
            "空盘胜率应贴 0.5，实际 {}（nodes={}）",
            r.win_rate,
            r.nodes
        );
        assert!(r.nodes > 100, "预算内只跑了 {} 次 playout", r.nodes);
    }

    /// 空盘首选必须稳定落在中心，且不随搜索量漂移。
    ///
    /// 这是 PUCT 先验的回归守卫。没有先验时，随机 playout 分辨不出空盘点位的优劣
    /// （实测前五名的 q 挤在 0.50~0.53，标准误约 0.018），`argmax(visits)` 等于在
    /// 噪声里取最大值——同一个种子下首选会随预算在 (6,2)/(1,5)/(5,2)/(4,4)/(2,4)/
    /// (1,2) 之间乱跳，其中好几个在二线，用户看到的是「AI 第一手像乱下」。
    ///
    /// 三个预算＝三档搜索量。修复前这三次大概率给出互不相同的着法；断言取中心
    /// 3×3 这个宽松判据，是为了别把「先验把范围收进中心」误判成「必须精确等于
    /// 天元」——(4,4) 与 (3,4) 在 9 路空盘上都是合理开局。
    #[test]
    fn go9_empty_first_move_is_stable() {
        let picks: Vec<(u8, u8)> = [200u32, 600, 1500]
            .iter()
            .map(|&ms| {
                let s = go(9);
                analyze(&s, 9, Stone::Black, test_budget(ms), true)
                    .best_move
                    .expect("空盘必须有可下之处")
            })
            .collect();
        for &(x, y) in &picks {
            assert!(
                (3..=5).contains(&x) && (3..=5).contains(&y),
                "9 路空盘首选跑到中心 3×3 之外（先验失效？）：{picks:?}"
            );
        }
    }

    /// 短预算下的首选不能由棋盘扫描序决定。
    ///
    /// `legal_moves` 若不洗牌，空点表按 `Vertex::all()` 的下标序生成，末位恒是
    /// `from_coords(8, 8)`（即 (8,8)），而 `iterate` 从末位 `pop()`——0ms 预算只跑
    /// 1 次迭代、根节点只有这 1 个子节点，首选就会被钉死在 (8,8)。
    ///
    /// 这条断言与机器速度、playout 数无关，是确定性的；上面那条改成不判方位之后，
    /// 洗牌失效只能靠它兜住。
    #[test]
    fn short_budget_move_is_not_scan_order() {
        let s = go(9);
        let r = analyze(&s, 9, Stone::Black, 0, true);
        assert_eq!(r.nodes, 1, "0ms 预算应恰好跑 1 次迭代");
        let got = r.best_move.expect("空盘必有可下之处");
        assert_ne!(got, (8, 8), "首选被扫描序钉死：(8,8) 是空点表末位");
        assert_ne!(got, (0, 0), "首选被扫描序钉死：(0,0) 是空点表首位");
        let mut probe = s.clone();
        assert!(probe.try_play(Move::Place(Coord::new(got.0, got.1))).is_ok());
    }

    /// 每一步都必须能被 core 接受：AI 与 core 用的是两套规则实现，返回前必须复验。
    #[test]
    fn best_move_is_always_legal_in_core() {
        let mut cases: Vec<GameState> = Vec::new();
        cases.push(go(9));

        let mut mid = go(9);
        for &(x, y) in &[(4, 4), (2, 2), (4, 2), (3, 3), (5, 5), (2, 4), (6, 6), (1, 1)] {
            place(&mut mid, x, y);
        }
        cases.push(mid);

        // 劫形（core 的简单劫）：黑提子成劫后轮白，白立即回提在 core 里是非法手。
        let mut ko = go(9);
        for &(x, y) in &[
            (0, 2), (0, 1), (2, 2), (2, 1), (1, 3), (1, 0), (7, 7), (7, 8), (8, 7), (8, 8),
            (6, 7), (1, 2), (1, 1),
        ] {
            place(&mut ko, x, y);
        }
        assert_eq!(ko.ko_point, Some(Coord::new(1, 2)));
        cases.push(ko);

        let mut s13 = go(13);
        for &(x, y) in &[(6, 6), (3, 3), (6, 3), (4, 4)] {
            place(&mut s13, x, y);
        }
        cases.push(s13);

        for (i, st) in cases.iter().enumerate() {
            let want = st.to_move;
            let r = analyze(st, st.kind.size() as u8, want, 300, true);
            let (x, y) = r.best_move.unwrap_or_else(|| panic!("case {i}: 必须有可下之处"));
            let mut probe = st.clone();
            assert!(
                probe.try_play(Move::Place(Coord::new(x, y))).is_ok(),
                "case {i}: AI 给出 core 拒绝的着法 ({x},{y})"
            );
        }
    }

    /// 提死子局面：白 4×4 块只剩一气 (5,4)，黑提掉后净赚 16 子。
    ///
    /// 为什么块要开得这么大：这个测试问的是「AI 会不会去提」，而 debug 构建下
    /// playout 慢约 70 倍（实测 9 路 1000ms：release 3.7 万次、debug 五百次出头），
    /// 块小到只赚 9 子时，提与不提的胜率差（约 0.8 对 0.78）会淹在几百次 playout
    /// 的噪声里。块大到「提了稳赢、不提稳输」，几百次 playout 就足以分辨。
    ///
    /// 走子顺序刻意排成「围方先围、被围方后填」：块若先摆好再去围，块内子落子时
    /// 无气即自杀，这个局面在 core 里根本构造不出来。
    ///
    /// `swap` 为真时整体黑白对调——黑先停一手把先手让给白，之后同一串坐标就会以
    /// 对调的身份落下。
    fn atari_position(swap: bool) -> GameState {
        let mut s = go(9);
        if swap {
            play(&mut s, Move::Pass);
        }
        // 4×4 块的 16 个外邻点，最后一点 (5,4) 留作猎物的一气。
        let ring: [(u8, u8); 16] = [
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (1, 5),
            (2, 5),
            (3, 5),
            (4, 5),
            (5, 1),
            (5, 2),
            (5, 3),
            (5, 4),
        ];
        // 块内 16 点，(4,4) 留到最后一步才填。
        let inner: [(u8, u8); 16] = [
            (1, 1),
            (2, 1),
            (3, 1),
            (1, 2),
            (2, 2),
            (3, 2),
            (1, 3),
            (2, 3),
            (3, 3),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 1),
            (4, 2),
            (4, 3),
            (4, 4),
        ];
        // 每轮：围方占一个围点，被围方填一个块内点。15 轮后块还剩 (4,4) 与 (5,4) 两气。
        for i in 0..15 {
            place(&mut s, ring[i].0, ring[i].1);
            place(&mut s, inner[i].0, inner[i].1);
        }
        // 围方停一手让轮手回到自己（单 pass，不会触发双 pass 终局），
        // 被围方补上块内最后一点 —— 块随之只剩 (5,4) 这一口气。
        play(&mut s, Move::Pass);
        place(&mut s, inner[15].0, inner[15].1);
        s
    }

    /// 提死子：白块只剩一气，黑提掉后凭空多出 16 子。AI 必须提。
    #[test]
    fn captures_group_in_atari() {
        let s = atari_position(false);
        assert_eq!(s.to_move, Stone::Black);
        // 前提自检：白块确实只剩 (5,4) 一气，且黑下在那里能提。
        let mut probe = s.clone();
        let eff = probe.try_play(Move::Place(Coord::new(5, 4))).unwrap();
        assert_eq!(eff.captured.len(), 16, "前提局面不对：白块不是 16 子一气");

        let r = analyze(&s, 9, Stone::Black, test_budget(1000), true);
        assert_eq!(
            r.best_move,
            Some((5, 4)),
            "AI 没有提掉只剩一气的白块（nodes={} win={:.3}）",
            r.nodes,
            r.win_rate
        );
    }

    /// 测试用的思考预算。debug 构建下 playout 慢约 70 倍，同一个断言需要的时间也差
    /// 同样倍数，否则测的是「编译器优化开没开」而不是引擎行为。
    ///
    /// 只放大预算，不用 `#[cfg]` 把断言砍掉：两种 profile 下断言强度必须一致。
    fn test_budget(release_ms: u32) -> u32 {
        if cfg!(debug_assertions) { release_ms * 12 } else { release_ms }
    }

    /// 预算超时的容差：把 `DEADLINE_CHECK_EVERY` 次 playout 的**实测**耗时当作尺子。
    ///
    /// 不写死毫秒上界。`cargo test` 默认并行开好几个用例，重活互相抢 CPU，墙上时间能
    /// 翻几倍——写死的上界于是随机变红（本仓 `gomoku::time_budget_is_respected` 与
    /// 上一版 `go19_empty_returns_in_budget` 都是这么飘的）。这里的尺子取自本次搜索
    /// 自己测出的单次 playout 耗时，尺子与被测对象在同一台机器、同一时刻，负载对两者
    /// 同向作用，比值就稳定。
    ///
    /// 它检验的是 [`DEADLINE_CHECK_EVERY`] 那条设计主张：两次检查之间最多多做这么多
    /// 次 playout。反过来，若常量被改得很大，这条断言会跟着放宽——那种情况由下面的
    /// 墙上时间兜底抓。
    fn budget_slack(r: &AnalyzeResult) -> f64 {
        let per_playout_ms = r.elapsed_ms as f64 / r.nodes.max(1) as f64;
        2.0 * DEADLINE_CHECK_EVERY as f64 * per_playout_ms + 20.0
    }

    /// 时间预算：搜索自报的耗时不得超过「预算 + 一个 deadline 周期」的余量。
    ///
    /// 先空跑一次把 gamma 表建起来：那是进程级一次性开销，不该算进「一次搜索要多久」。
    #[test]
    fn respects_time_budget() {
        let _ = gammas();
        let s = go(9);
        let budget = 1000u32;
        let t = Instant::now();
        let r = analyze(&s, 9, Stone::Black, budget, true);
        let wall = t.elapsed().as_millis() as u64;
        let slack = budget_slack(&r);
        assert!(
            r.elapsed_ms as f64 <= f64::from(budget) + slack,
            "budget={budget}ms 自报 {}ms（nodes={}），超过上界 {:.0}ms",
            r.elapsed_ms,
            r.nodes,
            f64::from(budget) + slack
        );
        // 墙上时间只负责兜「没有卡死」：它还要摊上建盘、重放与调度抖动，给 4 倍余量。
        assert!(
            wall <= u64::from(budget) * 4 + 2000,
            "budget={budget}ms 墙上跑了 {wall}ms"
        );
    }

    /// 胜率视角一致性：同一局面换个视角，胜率必须互补。
    ///
    /// 两次搜索的随机种子与局面完全相同，树也相同，所以差的只可能是视角折算。
    /// 容差留给 deadline 抖动（两次跑到的 playout 数不会完全一样）。
    #[test]
    fn win_rate_viewpoint_is_complementary() {
        let mut s = go(9);
        for &(x, y) in &[(4, 4), (2, 2), (4, 2), (3, 3), (5, 5), (2, 4)] {
            place(&mut s, x, y);
        }
        let b = analyze(&s, 9, Stone::Black, 500, false);
        let w = analyze(&s, 9, Stone::White, 500, false);
        assert!(
            (b.win_rate + w.win_rate - 1.0).abs() < 0.1,
            "黑白视角胜率不互补：black={} white={}",
            b.win_rate,
            w.win_rate
        );
    }

    /// 黑白互换（连局面一起镜像）后，同视角的胜率应接近原值。
    ///
    /// 镜像 ＝ 棋子颜色对调 + 轮手对调；再取对调的视角，两次搜索看到的应当是同一场棋。
    /// 视角折算写反（`my_color` 或节点的胜负方向弄反）都会让结果跑到 1 减原值去，
    /// 所以这里挑一个 0.8 上下的**决定性**局面：容差才敢放宽到 0.3 而仍然能抓到折算错误。
    ///
    /// 容差不能更小：go_game_board 的 `playout_score` 里钉死了 komi 6.5 且盘面
    /// （私有字段，无 setter），黑方天然背着约 7 目的贴目差，镜像局面本来就不会
    /// 得到完全相同的数。空盘实测黑 0.46 / 白 0.54，局面越尖锐这个差越大。
    #[test]
    fn win_rate_mirrors_with_colors() {
        let a = analyze(&atari_position(false), 9, Stone::Black, test_budget(400), false);
        let b = analyze(&atari_position(true), 9, Stone::White, test_budget(400), false);
        assert!(a.win_rate > 0.6 && b.win_rate > 0.6, "决定性局面没判成优势：{} / {}", a.win_rate, b.win_rate);
        assert!(
            (a.win_rate - b.win_rate).abs() < 0.3,
            "镜像局面的同视角胜率应接近：{} vs {}",
            a.win_rate,
            b.win_rate
        );
    }

    /// 19 路空盘：不 panic、在预算内返回、给出的着法合法。
    ///
    /// 这里**不**断言首选的方位：19 路根节点有 362 个候选，几百次 playout 平摊下来
    /// 每个候选只有个位数访问，方位纯属噪声。9 路那边同样断不了方位——原因见
    /// `go9_empty_returns_a_sane_result` 的注释，随机 playout 分辨不出空盘点位优劣。
    #[test]
    fn go19_empty_returns_in_budget() {
        // 先建 gamma 表：它是进程级一次性开销，混进计时会误判成「超预算」。
        let _ = gammas();
        let s = go(19);
        let budget = 500u32;
        let t = Instant::now();
        let r = analyze(&s, 19, Stone::White, budget, true);
        let wall = t.elapsed().as_millis() as u64;
        let slack = budget_slack(&r);
        assert!(
            r.elapsed_ms as f64 <= f64::from(budget) + slack,
            "19 路 budget={budget}ms 自报 {}ms（nodes={}），超过上界 {:.0}ms",
            r.elapsed_ms,
            r.nodes,
            f64::from(budget) + slack
        );
        assert!(
            wall <= u64::from(budget) * 4 + 2000,
            "19 路 budget={budget}ms 墙上跑了 {wall}ms"
        );
        let (x, y) = r.best_move.expect("空盘必须有可下之处");
        let mut probe = s.clone();
        assert!(
            probe.try_play(Move::Place(Coord::new(x, y))).is_ok(),
            "19 路给出 core 拒绝的着法 ({x},{y})"
        );
        assert!((0.0..=1.0).contains(&r.win_rate) && r.win_rate.is_finite());
    }

    /// 纯胜率模式（want_move=false）照样搜索，但不返回着法。
    #[test]
    fn win_rate_only_skips_move() {
        let mut s = go(9);
        place(&mut s, 4, 4);
        place(&mut s, 5, 5);
        let r = analyze(&s, 9, Stone::Black, 200, false);
        assert_eq!(r.best_move, None);
        assert!(r.nodes > 0, "want_move=false 也必须照常搜索");
    }

    /// 极短预算（0ms）也要给出结构完整的结果，不能 panic 或返回 NaN。
    #[test]
    fn zero_budget_is_degenerate_but_sane() {
        let s = go(9);
        let r = analyze(&s, 9, Stone::Black, 0, true);
        assert!(r.win_rate.is_finite() && (0.0..=1.0).contains(&r.win_rate));
        assert!(r.nodes >= 1);
    }

    /// 劫局里返回的着法必须被 core 接受，且绝不能是劫点（core 会以 `RuleError::Ko` 拒掉）。
    #[test]
    fn never_returns_the_ko_point() {
        // 与 core 的 `go_ko_forbids_immediate_recapture` 同一串「风车」劫形。
        let mut s = go(9);
        for &(x, y) in &[
            (0, 2), (0, 1), (2, 2), (2, 1), (1, 3), (1, 0), (7, 7), (7, 8), (8, 7), (8, 8),
            (6, 7), (1, 2), (1, 1),
        ] {
            place(&mut s, x, y);
        }
        assert_eq!(s.ko_point, Some(Coord::new(1, 2)));

        let r = analyze(&s, 9, Stone::White, 300, true);
        let (x, y) = r.best_move.expect("劫形下仍有大量合法着法");
        assert_ne!((x, y), (1, 2), "劫点被当成最佳着法返回");
        let mut probe = s.clone();
        assert!(
            probe.try_play(Move::Place(Coord::new(x, y))).is_ok(),
            "AI 给出 core 拒绝的着法 ({x},{y})"
        );
    }
}
