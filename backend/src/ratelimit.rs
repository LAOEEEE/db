//! 简单限流器：防止某个 IP 疯狂请求打爆服务器
//!
//! 实现思路（固定窗口算法）：
//! 给每个 IP 记一个"窗口"，窗口里有一个计数器。
//! 只要请求频率没超，就把计数器 +1 放行；
//! 计数器到达上限就拒绝（返回 429）；窗口时间一到，计数器清零重来。
//!
//! 我们用的是比较简单的版本：**10 秒内**最多 **120 次**请求（可在环境变量配置）。
//! 对于刮刮乐这种"刮一格才发一次请求"的场景完全够用。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 限流器。
///
/// 字段说明：
/// - `capacity`：一个窗口内允许的最大请求次数
/// - `window`：窗口的时间长度（这里是 10 秒）
/// - `buckets`：每个 IP 一个"桶"，桶里记着（窗口开始时间，已经用了多少次）
///
/// `Mutex<...>`（互斥锁）的作用：
/// 多线程同时读写 buckets 会冲突，所以用 Mutex 保证同一时刻只有一个人能改。
/// 为什么这里用 Mutex 而不是 RwLock：因为每次请求几乎都是"改"（加计数），
/// 读的机会少，用 Mutex 更简单直接。
///
/// `Instant` 是"某个时刻"，用来算窗口还剩下多久。
#[derive(Debug)]
pub struct RateLimiter {
    capacity: u32,
    window: Duration,
    buckets: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
    /// 创建限流器。capacity = 一个窗口内允许几次，window = 窗口多长。
    pub fn new(capacity: u32, window: Duration) -> Self {
        RateLimiter {
            capacity,
            window,
            // 刚开始还没有任何 IP 的记录，所以是空 HashMap
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 检查这个 IP 这次请求允不允许通过。
    /// 返回 true = 放行，false = 限流（拒绝）。
    ///
    /// 整体逻辑：
    /// 1. 拿锁（buckets 被多个线程共享，先锁上才安全）
    /// 2. 如果记录的 IP 太多了，先顺手清理一下过期窗口，避免内存无限涨
    /// 3. 查这个 IP 的桶：如果没有记录，就新建一个（此刻窗口开始）
    /// 4. 如果窗口时间已过，就把窗口重置：记成"现在开始，用掉 1 次"，放行
    /// 5. 如果还在窗口内但次数已达上限，拒绝
    /// 6. 否则次数 +1，放行
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        // lock() 拿锁。expect 表示"锁坏了就直接崩溃"——
        // 如果一个线程 panic 时还握着锁，Mutex 会进入"中毒"状态。
        // 对本项目来说，锁被毒死说明程序已经出大问题了，崩溃反而好排查
        let mut buckets = self.buckets.lock().expect("rate limiter lock poisoned");
        // 防御措施：如果记录的 IP 超过了 4096 个，就清掉所有过期窗口。
        // 否则攻击者伪造海量不同 IP，HashMap 会无限膨胀吃掉内存
        if buckets.len() > 4096 {
            // retain：留下"窗口还没结束"的，删掉已经结束的
            buckets.retain(|_, (started, _)| now.duration_since(*started) < self.window);
        }
        // entry(ip).or_insert((now, 0))：取这个 IP 的桶。
        // 如果没有，就创建并初始化为 (now, 0) = 窗口从此刻开始、还没用过次数。
        // or_insert 返回的是可变引用，可以直接改它
        let entry = buckets.entry(ip).or_insert((now, 0));
        // 窗口时间到了：上一窗口作废，从此刻重新开始算，这次算第 1 次
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 1);
            return true;
        }
        // 还在窗口内但次数用完了：拒绝
        if entry.1 >= self.capacity {
            return false;
        }
        // 正常情况：次数 +1，放行
        entry.1 += 1;
        true
    }
}

/// 单元测试模块。
/// `#[cfg(test)]` 的意思是：这段代码**只在跑 cargo test 时**编译，
/// 打正式包（cargo build --release）时完全不会带进二进制里。
/// 所以可以放心写测试代码，不用担心拖累程序体积。
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// 测试：同一个 IP，次数没超就放行，超了就开始拒绝
    #[test]
    fn allows_up_to_capacity_then_blocks() {
        // 造一个容量 3、窗口 10 秒的限流器
        let limiter = RateLimiter::new(3, Duration::from_secs(10));
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        // 前 3 次应该都放行
        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        assert!(limiter.check(ip));
        // 第 4 次该被拒绝了
        assert!(!limiter.check(ip));
    }

    /// 测试：不同 IP 各算各的，互不影响
    #[test]
    fn windows_are_independent_per_ip() {
        let limiter = RateLimiter::new(1, Duration::from_secs(10));
        // 两个不同 IP 各自第一次都放行
        assert!(limiter.check(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(limiter.check(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
        // 第一个 IP 已经用掉了它唯一的 1 次，第二次应该被拒
        assert!(!limiter.check(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }
}
