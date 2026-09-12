//! HTTP 接口层：接收前端请求，调用游戏逻辑，把结果返回给前端
//!
//! 这是服务器对外"露脸"的部分。浏览器发来的每个请求，
//! 都会先被 axum 路由到这里对应的函数，由这里决定怎么处理。
//!
//! 完整的请求-响应链条：
//! 浏览器 → HTTP 请求 → axum 路由 → 本文件某个 handler
//!       → 拿锁操作 GameStore / Game → 组装 JSON 响应 → 返回浏览器
//!
//! 本文件还用中间件实现了全局限流，保护服务器不被请求刷爆。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{
    ConnectInfo, DefaultBodyLimit, Json as AxumJson, Request, State,
};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::config::GameConfig;
use crate::models::{
    FinishRequest, FinishResponse, Game, GameError, NewCardResponse, RevealRequest, RevealResponse,
};
use crate::ratelimit::RateLimiter;
use crate::state::SharedStore;

/// 路由里所有 handler 要共享的"应用状态"。
/// 元组（tuple）里有两个东西：
/// 1. SharedStore：卡片仓库（见 state.rs），所有请求都要读写它
/// 2. Arc<GameConfig>：只读的游戏配置，所有请求都要查它（概率表等）
///
/// Arc 让两个字段能被多个线程安全共享。
///
/// 起别名的意义：`State<AppState>` 比 `State<(SharedStore, Arc<GameConfig>)>` 短太多。
pub type AppState = (SharedStore, Arc<GameConfig>);

/// 组装整个 /api 路由（把 URL 路径和函数一一对上）。
///
/// 入参：
/// - `state`：共享状态，会通过 .with_state 绑定进路由
/// - `rate_capacity`：限流器每个窗口允许的次数，从环境变量读来
///
/// 什么是"中间件"（middleware）：
/// 一个"夹在请求和真正的 handler 之间"的处理层。
/// 请求进来先过中间件（限流），通过后才进真正的函数；
/// 响应返回时再经过中间件回去。常用于打日志、鉴权、限流这类
/// "每个接口都要做"的事。
pub fn router(state: AppState, rate_capacity: u32) -> Router {
    // 先造好限流器：rate_capacity 次 / 10 秒
    let rate_limiter = Arc::new(RateLimiter::new(rate_capacity, Duration::from_secs(10)));
    Router::new()
        // 五个接口的路径与处理函数对应关系
        .route("/health", get(health))                 // GET  健康检查
        .route("/config", get(get_config))             // GET  拿公开配置
        .route("/game/new", post(new_game))            // POST 新建卡
        .route("/game/reveal", post(reveal))           // POST 刮一格
        .route("/game/finish", post(finish))           // POST 结算
        // 限制请求体最大 4KB。刮奖请求体很小，超了基本就是恶意请求
        .layer(DefaultBodyLimit::max(4096))
        // 挂上限流中间件。from_fn_with_state 的意思是：
        // 这个中间件也需要访问限流器，所以把 Arc<RateLimiter> 塞给它当状态
        .layer(from_fn_with_state(rate_limiter, rate_limit))
        // 把 (仓库, 配置) 绑进路由，之后 handler 用 State<AppState> 就能取到
        .with_state(state)
}

/// 限流中间件本体。
///
/// 参数是 axum 中间件的固定模板：
/// - `State(limiter)`：从路由里取出绑定的限流器
/// - `ConnectInfo(addr)`：取出客户端的 IP（这就是"按 IP 限流"的依据）
/// - `request`：当前的 HTTP 请求
/// - `next`：下一个环节（下一个中间件或真正的 handler）
///
/// 如果限流器说"放行"，就调用 next.run(request) 放它过去；
/// 否则直接返回 429（Too Many Requests），请求根本到不了 handler。
async fn rate_limit(
    State(limiter): State<Arc<RateLimiter>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if limiter.check(addr.ip()) {
        next.run(request).await
    } else {
        // json! 宏：直接写 JSON 字面量，省得手写转义
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "rate_limited",
                "message": "操作过于频繁，请稍后再试",
            })),
        )
            .into_response() // 把 (状态码, JSON) 转成真正的 HTTP 响应
    }
}

/// 健康检查：GET /api/health
/// 返回 {"status": "ok"}，部署脚本用它确认服务活着。
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// 公开配置：GET /api/config
/// 只返回前端需要的那两个值（每日次数、刮开阈值）。
/// 注意：**不**返回奖级和概率表——那是核心机密，只在服务器内存里。
///
/// 参数写法 `State((_store, config))` 的拆解：
/// AppState 是个元组，用模式匹配把它拆开。
/// `_store` 前加下划线：这个 handler 用不到仓库，但解构必须把它取出来，
/// 下划线是 Rust 的约定，表示"这个变量故意不用"。
/// `config` 是 Arc<GameConfig>，要访问里面的字段就 config.daily_limit（Arc 自动解引用）。
pub async fn get_config(
    State((_store, config)): State<AppState>,
) -> Json<serde_json::Value> {
    Json(json!({
        "daily_limit": config.daily_limit,
        "reveal_threshold": config.reveal_threshold,
    }))
}

/// 新建卡片：POST /api/game/new
///
/// 流程：用配置发一张新卡（见 game.rs 的 Game::create）→
/// 放进仓库 → 返回 card_id 和格子编号（不含图案）。
///
/// 返回类型 `Result<Json<NewCardResponse>, ApiError>`：
/// Ok = 成功的 JSON 响应，Err = 转成错误响应返回给前端。
/// `?` 运算符在这里的作用：如果写锁失败（unwrap 不到），
/// 立刻 return Err(ApiError::internal())，不往下走。
pub async fn new_game(
    State((store, config)): State<AppState>,
) -> Result<Json<NewCardResponse>, ApiError> {
    let game = Game::create(&config);
    let response = game.to_new_card_response();
    {
        // 一对大括号：让写锁尽快释放（离开作用域就自动 drop）。
        // 锁拿得越久，其他请求等待的时间越长，所以用块把临界区圈小一点。
        let mut guard = store.write().map_err(|_| ApiError::internal())?;
        guard.insert(game);
    } // 到这里写锁自动释放
    tracing::info!(card_id = %response.card_id, "new game");
    Ok(Json(response))
}

/// 刮一格：POST /api/game/reveal
///
/// 入参 payload 的类型 `Result<AxumJson<RevealRequest>, JsonRejection>`：
/// axum 先把请求体解析成 RevealRequest，如果 JSON 格式不对，
/// 会给出一个 JsonRejection（解析失败详情）。
/// 用 Result 包住，方便统一转成"请求格式不对"的 400 错误。
pub async fn reveal(
    State((store, _config)): State<AppState>,
    payload: Result<AxumJson<RevealRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<RevealResponse>, ApiError> {
    // map_err：把"解析失败"转换成 ApiError（返回 400）
    let AxumJson(req) = payload.map_err(ApiError::bad_request)?;
    // 防御：card_id 不能是空字符串（否则查仓库一定找不到，纯浪费）
    if req.card_id.is_empty() {
        return Err(ApiError::bad_request_str("card_id 不能为空"));
    }
    // 拿写锁。写锁而不是读锁，是因为 reveal_cell 要修改格子状态
    let mut guard = store.write().map_err(|_| ApiError::internal())?;
    // 按 id 找卡。找不到就转成 404 错误（CardNotFound → card_not_found）
    let game = guard
        .game_mut(&req.card_id)
        .ok_or(ApiError::from(GameError::CardNotFound))?;
    // 真正执行"刮开一格"。结果可能是 Ok(图案) 或 Err(已刮过/越界等)
    let response = game.reveal_cell(req.cell_id)?;
    Ok(Json(response))
}

/// 结算：POST /api/game/finish
///
/// 注意第 112 行：结算成功后顺手调用 record_finish 更新统计数字
/// （总中奖数、总发分），以后可以做排行榜之类的功能。
pub async fn finish(
    State((store, _config)): State<AppState>,
    payload: Result<AxumJson<FinishRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<FinishResponse>, ApiError> {
    let AxumJson(req) = payload.map_err(ApiError::bad_request)?;
    if req.card_id.is_empty() {
        return Err(ApiError::bad_request_str("card_id 不能为空"));
    }
    let mut guard = store.write().map_err(|_| ApiError::internal())?;
    let game = guard
        .game_mut(&req.card_id)
        .ok_or(ApiError::from(GameError::CardNotFound))?;
    let response = game.finish()?;
    guard.record_finish(response.reward);
    tracing::info!(card_id = %req.card_id, win = response.win, reward = response.reward, "game finished");
    Ok(Json(response))
}

/// 统一的 API 错误类型。
/// 三个字段配合起来，能生成一个规范化的错误 JSON：
/// { "error": "错误码", "message": "给人看的说明" }
pub struct ApiError {
    status: StatusCode,     // HTTP 状态码（400 / 404 / 409 / 500……）
    code: &'static str,     // 机器可读的错误码（英文短词），前端可据此判断错误类型
    message: String,        // 用户可读的错误说明（中文）
}

impl ApiError {
    /// 服务器内部错误：500。
    /// 什么时候会走到这：拿锁失败等"代码自身出问题"的情况。
    fn internal() -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "服务器内部错误".to_string(),
        }
    }

    /// 请求格式错误：400。
    /// `err.body_text()` 拿到 axum 解析失败时给出的具体原因（哪不对）。
    fn bad_request(err: axum::extract::rejection::JsonRejection) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: err.body_text(),
        }
    }

    /// 请求格式错误（手动版）：400。
    /// 前面那个是"axum 解析失败自动转"，这个是"我们自己发现参数不对"，
    /// 比如 card_id 为空。message 由我们直接给定。
    fn bad_request_str(message: &str) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.to_string(),
        }
    }
}

/// 让"游戏错误"能自动转换成 API 错误。
///
/// 这就是 `game.reveal_cell(req.cell_id)?` 里 `?` 能工作的另一半原因：
/// reveal_cell 返回 GameError，而函数要返回 ApiError，
/// Rust 看到 GameError 实现了 `From<GameError> for ApiError`，
/// 就自动调用这里的转换逻辑，把 GameError 变成对应的 ApiError。
impl From<GameError> for ApiError {
    fn from(err: GameError) -> Self {
        // 一一对应：每种游戏错误 → 对应的 HTTP 状态码和文案
        match err {
            // 404：卡不存在（可能被清理了，或根本没创建）
            GameError::CardNotFound => ApiError {
                status: StatusCode::NOT_FOUND,
                code: "card_not_found",
                message: "刮刮卡不存在或已过期".to_string(),
            },
            // 400：格子编号不在 0~8 里
            GameError::CellOutOfRange => ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_cell",
                message: "格子编号无效".to_string(),
            },
            // 409：冲突。这格已经刮过了，重复操作
            GameError::AlreadyRevealed => ApiError {
                status: StatusCode::CONFLICT,
                code: "already_revealed",
                message: "该格子已经刮开".to_string(),
            },
            // 409：冲突。卡已经结算，想重复领奖
            GameError::AlreadyFinished => ApiError {
                status: StatusCode::CONFLICT,
                code: "already_finished",
                message: "该刮刮卡已结算，不能重复领取".to_string(),
            },
        }
    }
}

/// 把 ApiError 变成真正的 HTTP 响应。
///
/// `IntoResponse` 是 axum 的核心 trait（接口）：
/// 所有"能被当成 HTTP 响应返回的东西"都要实现它。
/// Json、String、StatusCode 等 axum 已经帮你实现了，
/// 这里给自定义的 ApiError 也实现一个，
/// 这样任何返回 ApiError 的函数，axum 都知道怎么发给浏览器。
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 统一包装成 { "error": ..., "message": ... }
        let body = Json(json!({
            "error": self.code,
            "message": self.message,
        }));
        // (状态码, JSON) 元组也是 IntoResponse，直接转
        (self.status, body).into_response()
    }
}
