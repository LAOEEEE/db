//! 程序入口：启动服务器
//!
//! 整个程序的起点（main 函数）就藏在这里。它负责在启动时把一切拼装好：
//! 1. 初始化日志系统
//! 2. 从环境变量读配置（监听地址、web 目录、配置文件路径……）
//! 3. 加载游戏配置
//! 4. 创建卡片仓库（空的）
//! 5. 组装 HTTP 路由（/api/* + 静态文件）
//! 6. 监听端口，开始服务，直到被终止

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use axum::Router;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

// 声明这个二进制用到的其他模块。
// 每个 mod 对应 backend/src/ 下的一个同名文件：
// api.rs / config.rs / game.rs / models.rs / ratelimit.rs / state.rs
mod api;
mod config;
mod game;
mod models;
mod ratelimit;
mod state;

// 引入下面会用到的两个类型，省得每次写全路径
use config::GameConfig;
use state::GameStore;

/// 程序入口。
///
/// `#[tokio::main]` 是什么：
/// axum 是异步框架，main 函数本身是同步的。
/// 这个宏把 main 包装一下，自动帮你启动 tokio 的异步运行时（线程池），
/// 让里面的异步代码（await 等）能跑起来。
///
/// `async fn main() -> Result<(), Box<dyn std::error::Error>>`：
/// - `async`：函数体里可以写异步操作
/// - 返回 `Result`：Ok(()) 表示正常退出；Err 则打印错误并让程序退出。
///   `Box<dyn std::error::Error>` 是"随便什么错误"的通用类型，
///   这样函数里任何 `?` 抛出的错误都能兜住，不用写具体错误类型。
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 1. 初始化日志系统 ----
    // tracing 是本项目的日志库。日志分级别：error / warn / info / debug / trace。
    // EnvFilter 让日志级别可通过环境变量 RUST_LOG 控制（比如 RUST_LOG=debug），
    // 没设置就用 info 级别。
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact() // 紧凑格式，日志更短
        .init();

    // ---- 2. 从环境变量读监听地址 ----
    // 读 SCRATCH_BIND，例如 "0.0.0.0:3000"。
    // 读不到或解析失败就用默认值 0.0.0.0:3000（对所有网卡开放，树莓派上能被局域网访问）。
    // 0.0.0.0 = 本机的所有 IP 地址，所以局域网内任何设备都能连上。
    let bind: SocketAddr = std::env::var("SCRATCH_BIND")
        .ok()                  // Result 变 Option：读到就 Some(字符串)，没有就 None
        .and_then(|v| v.parse().ok()) // 字符串解析成 SocketAddr，解析失败就 None
        .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 3000))); // 兜底默认值

    // 前端静态文件目录。不指定就用当前目录下的 "web" 文件夹
    let web_dir = std::env::var("SCRATCH_WEB_DIR").unwrap_or_else(|_| "web".to_string());
    // 游戏配置文件路径。不指定就用 "config/game.json"
    let config_path =
        std::env::var("SCRATCH_CONFIG").unwrap_or_else(|_| "config/game.json".to_string());

    // ---- 3. 加载游戏配置 ----
    // Arc：配置要被多个线程的 handler 共享，包一层 Arc 就能安全共享（见 state.rs 注释）
    let config = Arc::new(GameConfig::load(&config_path)?);
    // 打日志：输出一共有几个奖级（"?" 是 Debug 打印占位符）
    tracing::info!(
        reward_tiers = config.rewards.len(),
        "game config loaded from {}",
        config_path
    );

    // 限流次数，默认 10 秒内每个 IP 最多 120 次请求
    let rate_capacity = std::env::var("SCRATCH_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);

    // ---- 4. 创建卡片仓库 ----
    // 新建一个空仓库，包上 Arc<RwLock<...>> 方便多线程共享。
    // 关于 RwLock：读写锁，见 state.rs 底部的详细解释。
    let store = Arc::new(RwLock::new(GameStore::default()));

    // ---- 5. 组装路由 ----
    let app = Router::new()
        // 所有 /api 开头的请求，交给 api.rs 里组装的路由处理
        .nest("/api", api::router((store, config), rate_capacity))
        // 其余所有请求（/、/assets/style.css 等）交给静态文件服务。
        // fallback_service 是一个"兜底"：ServeDir 先在 web 目录里找对应文件，
        // 找不到就返回 index.html（这就是单页应用的经典做法——
        // 不管访问什么路径，都先给你 index.html，页面再自己路由）。
        .fallback_service(
            tower_http::services::ServeDir::new(&web_dir)
                .fallback(tower_http::services::ServeFile::new(format!(
                    "{web_dir}/index.html"
                ))),
        );

    // ---- 6. 监听并开始服务 ----
    // 先绑定端口（.await 等待系统确认绑定成功）
    let listener = TcpListener::bind(bind).await?;
    tracing::info!("server listening on http://{bind}");
    // 正式开跑。into_make_service_with_connect_info 让每个请求都带上客户端 IP，
    // api.rs 里的限流中间件就是靠它拿到 IP 的。
    // 这一行会一直运行，直到进程被 Ctrl+C 或 kill 终止。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
