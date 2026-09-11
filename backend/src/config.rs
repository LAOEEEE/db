//! 游戏配置：加载与校验
//!
//! 游戏规则（奖级、概率、每日次数、刮开阈值）不是写死在代码里的，
//! 而是放在一个 JSON 文件里（默认 config/game.json），方便不改代码就调规则。
//! 这个模块负责：
//! 1. 从文件读取 JSON
//! 2. 把 JSON 变成 Rust 结构体（serde 反序列化）
//! 3. 校验规则合不合理（概率不能超过 1、符号不能为空……）
//! 4. 配置文件不存在时，用内置的默认配置兜底

use std::path::Path;

use serde::Deserialize;

/// 一个奖级（档位）。对应 config/game.json 里 rewards 数组里的一项。
///
/// 字段含义（也是 JSON 里的键）：
/// - `name`：奖级名称，如"一等奖"
/// - `symbol`：中奖符号，如 💎。中奖卡的三连就是这个符号
/// - `value`：中了给多少积分
/// - `probability`：抽中这个奖级的概率，范围 0~1，如 0.001 = 千分之一
#[derive(Debug, Clone, Deserialize)]
pub struct RewardTier {
    pub name: String,
    pub symbol: String,
    pub value: u32,
    pub probability: f64,
}

/// 整个游戏的配置。对应整个 config/game.json。
///
/// `#[serde(deny_unknown_fields)]` 是一个"保险丝"：
/// 如果 JSON 里写了结构体没有的字段（比如拼错了字段名），
/// serde 会直接报错，而不是悄悄忽略。
/// 这样能提前发现配置文件写错的问题，而不是运行到一半才出怪现象。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameConfig {
    /// 奖级列表，按数组顺序排列（一般从高到低）
    pub rewards: Vec<RewardTier>,
    /// 未中奖格子用的填充符号。中奖卡里除了三连之外，其余格子也从这里取
    pub filler_symbols: Vec<String>,
    /// 每天最多能抽几次。**0 表示不限次数**
    /// `#[serde(default = "default_daily_limit")]` 意思是：
    /// 如果 JSON 里没写这个键，就用 default_daily_limit() 函数的返回值（10）兜底
    #[serde(default = "default_daily_limit")]
    pub daily_limit: u32,
    /// 刮开面积达到多大比例就自动结算（如 0.9 = 刮开 90%）
    #[serde(default = "default_reveal_threshold")]
    pub reveal_threshold: f64,
}

/// 兜底值：每日次数默认 10 次
fn default_daily_limit() -> u32 {
    10
}

/// 兜底值：刮开阈值默认 0.4（40%）
fn default_reveal_threshold() -> f64 {
    0.4
}

/// 内置默认配置。
///
/// 给 `GameConfig` 实现 `Default` trait（特性/接口的意思），
/// 作用是让 `GameConfig::default()` 能直接得到一个合理的配置，
/// 也方便测试代码里快速造一个配置对象。
///
/// 这里的 JSON 字符串和 README 里展示的示例配置是同一份。
/// 调用 `serde_json::from_str` 把它解析成结构体，
/// `.expect(...)` 表示"这里不可能失败，如果失败了就直接崩溃"——
/// 因为这段 JSON 是我们自己写的固定字符串，语法不可能错。
impl Default for GameConfig {
    fn default() -> Self {
        let json = r#"{
            "rewards": [
                { "name": "一等奖", "symbol": "💎", "value": 1000, "probability": 0.001 },
                { "name": "二等奖", "symbol": "💰", "value": 100, "probability": 0.01 },
                { "name": "三等奖", "symbol": "🎁", "value": 20, "probability": 0.05 },
                { "name": "安慰奖", "symbol": "🍀", "value": 5, "probability": 0.2 }
            ],
            "filler_symbols": ["⭐", "🍒", "🔔", "🍋", "🍇", "🍉"],
            "daily_limit": 10,
            "reveal_threshold": 0.4
        }"#;
        serde_json::from_str(json).expect("builtin default config must parse")
    }
}

impl GameConfig {
    /// 从文件加载配置。
    ///
    /// `path: impl AsRef<Path>` 的含义：
    /// 参数写成"任何能当作路径用的类型"（字符串、Path、PathBuf 都行），
    /// 这样调用时可以传字符串字面量，也可以传 PathBuf，很灵活。
    ///
    /// 返回值 `Result<Self, ConfigError>`：
    /// Rust 处理"可能失败"的通用做法——要么是 `Ok(配置)`，要么是 `Err(错误)`。
    /// 调用方用 `?` 运算符就能在失败时立刻向上抛出错误。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        // 配置文件不存在时不算致命错误，警告一下然后用内置默认配置
        if !path.exists() {
            tracing::warn!("config file {} not found, using built-in defaults", path.display());
            return Ok(Self::default());
        }
        // 第一步：把文件内容整个读进内存（bytes 是一个 Vec<u8>，就是一串字节）
        let bytes = std::fs::read(path).map_err(ConfigError::Read)?;
        // 第二步：把字节解析成 Rust 结构体。
        // `?` 在这里的意思是：如果解析失败，就把 ConfigError::Parse 返回给调用方
        let config: GameConfig = serde_json::from_slice(&bytes).map_err(ConfigError::Parse)?;
        // 第三步：检查规则合不合理（比如概率和不能超 1）
        config.validate()
    }

    /// 校验配置是否合法。
    ///
    /// 注意入参是 `self`（不是 `&self`）：直接把配置"消费"掉，
    /// 校验通过就把自己原样返回（Ok(self)），不通过就返回错误。
    /// 写 `&self` 也行，但拿所有权可以少一层拷贝。
    ///
    /// 这里的校验逻辑都是"宁可启动时拒绝启动，也不带着坏配置运行"。
    fn validate(self) -> Result<Self, ConfigError> {
        // 奖级列表不能为空，否则根本没奖可发
        if self.rewards.is_empty() {
            return Err(ConfigError::Invalid("rewards 不能为空"));
        }
        // 填充符号至少要 3 个。
        // 因为中奖卡要求"恰好只有一条三连"，如果填充符号太少，
        // 随机生成时很容易凑出额外的三连，兜底代码也依赖至少 3 个不同符号
        if self.filler_symbols.len() < 3 {
            return Err(ConfigError::Invalid("filler_symbols 至少需要 3 个"));
        }
        // 刮开阈值必须在 5% 到 100% 之间。
        // 太小的阈值（比如 0）会导致用户刚刮一下就自动结算，体验很差
        if !(0.05..=1.0).contains(&self.reveal_threshold) {
            return Err(ConfigError::Invalid("reveal_threshold 必须在 0.05..=1 范围内"));
        }
        // 逐个检查奖级
        let mut sum = 0.0;
        for tier in &self.rewards {
            // 概率必须在 0~1 之间
            if !(0.0..=1.0).contains(&tier.probability) {
                return Err(ConfigError::Invalid("概率必须在 0..=1 范围内"));
            }
            // 中奖符号不能是空字符串（否则显示不出来）
            if tier.symbol.is_empty() {
                return Err(ConfigError::Invalid("奖级 symbol 不能为空"));
            }
            // 顺便累加所有概率
            sum += tier.probability;
        }
        // 所有奖级的概率之和不能超过 1。
        // 因为随机数只落在 0~1 之间，如果和超过 1 就会溢出出错。
        // 加 f64::EPSILON 是因为浮点数计算有微小误差，留一点点容错
        if sum > 1.0 + f64::EPSILON {
            return Err(ConfigError::Invalid("中奖概率之和不能超过 1"));
        }
        Ok(self)
    }
}

/// 配置加载可能出现的错误，分成三种情况。
///
/// Rust 习惯用枚举（enum）列举所有错误类型。
/// 每个变体后面带的数据就是出错时的详细信息：
/// - `Read`：读文件失败（文件不存在、没权限……），保存 std::io::Error
/// - `Parse`：JSON 语法或字段对不上，保存 serde_json::Error
/// - `Invalid`：配置内容不合规（概率超 1 等），保存一句中文说明
///
/// `#[derive(Debug)]` 让它能打印出来，方便调试。
#[derive(Debug)]
pub enum ConfigError {
    Read(std::io::Error),
    Parse(serde_json::Error),
    Invalid(&'static str),
}

/// 给 ConfigError 实现 Display（"怎么把错误显示成文字"）。
/// 这样 println!("{}", err) 或把它包进更大的错误时，能输出一句人话。
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Read(e) => write!(f, "读取配置失败: {e}"),
            ConfigError::Parse(e) => write!(f, "解析配置失败: {e}"),
            ConfigError::Invalid(msg) => write!(f, "配置无效: {msg}"),
        }
    }
}

/// 让 ConfigError 也能作为"标准错误"使用。
/// 这是 `?` 运算符工作的前提：main.rs 里的 `GameConfig::load(&config_path)?`
/// 要求 load 返回的错误类型实现 std::error::Error，
/// 这样它才能一路向上传播，最终被 main 函数返回。
impl std::error::Error for ConfigError {}
