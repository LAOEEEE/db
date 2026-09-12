//! 数据模型与 API 报文
//!
//! 这个文件是整个项目的"数据字典"：
//! 定义刮奖格长什么样，以及前端和后端之间收发 JSON 报文的结构。
//! 你在浏览器里看到的接口返回什么字段，都是由这里的结构体决定的。

use serde::{Deserialize, Serialize};

/// 一页里最多能放几个刮奖格（前端一页同样渲染这么多格）。
/// 需求：一页界面中可刮 5 次，所以这里是 5。
/// 用常量而不是到处写死数字，以后想改成一页 6 格只改这一处。
pub const MAX_PAGE_CELLS: usize = 5;

/// 一个刮奖格（一页里的一个"刮开位置"）。
///
/// 字段含义：
/// - `id`：格子编号，范围 0~4，用来定位"刮开的是哪一格"
/// - `amount`：这格中了多少钱，没中就是 0。
///   **刮开之前不会发给前端**，中奖结果完全由服务器掌握
/// - `label`：直接显示给用户的金额文本，比如 "$20"；没中奖是 "$0"
/// - `revealed`：这格是否已经被刮开（一开始全是 false）
///
/// 关于 `#[derive(...)]`：
/// 给结构体"白送"一些常用能力的魔法语法，写在括号里的都是要自动实现的东西：
/// - `Debug`：可以 println!("{:?}") 打印出来，方便调试
/// - `Clone`：可以复制出一份，不用担心所有权被拿走
/// - `Serialize`：能把结构体转成 JSON（Rust 的名字叫 serde 序列化）
/// - `Deserialize`：能从 JSON 转回结构体（serde 反序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub id: u8,
    pub amount: u32,
    pub label: String,
    pub revealed: bool,
}

/// 一页刮刮卡：最多装 MAX_PAGE_CELLS 个刮奖格。
/// 这一页**只在服务器内存里存在**，不会整页发给前端。
///
/// 字段含义：
/// - `id`：这一页的唯一编号，32 位十六进制字符串，由操作系统随机数生成
/// - `cells`：这一页里的刮奖格
/// - `finished`：是否已经结算。结算后就不能再刮也不能再结算了
/// - `created_at`：创建时间。服务器用来在页面太多时清理最旧的页面
///
/// 关于 `std::time::Instant`：
/// Rust 自带的"记录某个时刻"的类型，专门用来比较时间间隔，
/// 和钟表上的具体几点几分无关，只关心"过了多久"。
#[derive(Debug, Clone)]
pub struct Game {
    pub id: String,
    pub cells: Vec<Cell>,
    pub finished: bool,
    pub created_at: std::time::Instant,
}

/// 游戏中可能出现的错误，用枚举（enum）把错误分类。
///
/// 什么是枚举：列出"这件事可能出现的几种情况"。
/// 用的时候一个一个匹配，编译器能帮你检查有没有漏掉某种情况。
///
/// - `CardNotFound`：找不到这一页（可能没创建，也可能被清理掉了）
/// - `CellOutOfRange`：格子编号超出这一页的范围
/// - `AlreadyRevealed`：这格已经刮过了，不能重复刮
/// - `AlreadyFinished`：这一页已经结算了，不能再操作
///
/// `#[derive(...)]` 里的 PartialEq 和 Eq 让错误之间可以互相比较，
/// 测试里才能写 `assert_eq!(game.reveal_cell(9).unwrap_err(), GameError::CellOutOfRange)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameError {
    CardNotFound,
    CellOutOfRange,
    AlreadyRevealed,
    AlreadyFinished,
}

/// 新建一页时，返回给前端的"格子提示"。
/// 注意：只告诉前端有哪些格子，**不告诉金额**。
/// 金额要等用户真的刮开那一格，调 /api/game/reveal 才返回。
#[derive(Debug, Serialize)]
pub struct CellHint {
    pub id: u8,
}

/// 新建一页的请求（对应接口 POST /api/game/new）。
/// 这一页要几个刮奖格由前端决定（通常就是剩下的次数，最多 MAX_PAGE_CELLS）。
///
/// `#[serde(default)]` 的作用：请求体里没写 `count`（或者干脆发 `{}`）时，
/// 用 Option 的默认值 None，后端再按"一页满格"处理，不会解析失败。
#[derive(Debug, Deserialize)]
pub struct NewCardRequest {
    #[serde(default)]
    pub count: Option<u32>,
}

/// 新建一页的响应报文（对应接口 POST /api/game/new）。
/// 返回这一页的编号 + 每个格子的编号。
#[derive(Debug, Serialize)]
pub struct NewCardResponse {
    pub card_id: String,
    pub cells: Vec<CellHint>,
}

/// 刮开一格时前端发来的请求（对应接口 POST /api/game/reveal）。
/// 告诉服务器：哪一页、刮第几格。
///
/// 这里只用 `Deserialize` 不用 `Serialize`：
/// 因为这个结构体只会"从前端请求里读进来"，永远不会"发回给前端"。
#[derive(Debug, Deserialize)]
pub struct RevealRequest {
    pub card_id: String,
    pub cell_id: u8,
}

/// 刮开一格的响应：这一格是什么金额。
/// - `amount`：中了多少钱（0 = 没中），前端据此做高亮和累计
/// - `label`：显示文本，如 "$20" / "$0"
/// - `win`：是否中奖，前端不用自己判断 amount > 0
#[derive(Debug, Serialize)]
pub struct RevealResponse {
    pub cell_id: u8,
    pub amount: u32,
    pub label: String,
    pub win: bool,
}

/// 结算请求（对应接口 POST /api/game/finish）。
/// 只需要告诉服务器是哪一页，剩下的（中了多少）由服务器自己算。
#[derive(Debug, Deserialize)]
pub struct FinishRequest {
    pub card_id: String,
}

/// 结算响应：这一页最终的中奖结果。
/// - `win`：是否中奖（true / false）
/// - `reward`：这一页已刮开的格子合计中了多少钱，没中就是 0
/// - `message`：直接展示给用户的一句话（"本页共中奖 $20！"）
#[derive(Debug, Serialize)]
pub struct FinishResponse {
    pub win: bool,
    pub reward: u32,
    pub message: String,
}
