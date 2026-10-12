//! 「图」的渲染借回桌面端。
//!
//! 手机上不该再实现一套 Mermaid：桌面端已经有一份**唯一**的渲染入口
//! （`inknote/lib/mermaid.ts` 的 `renderDiagram`，负责唯一 id、串行队列、
//! 错误抑制——见 AGENTS.md 里那条「Mermaid 必须收口到一处」）。
//! 手机上再拉一份 3.5MB 的 mermaid 进二进制，除了重复还有版本漂移的风险。
//!
//! 所以走一条**请求-响应**的短桥：
//!
//! ```text
//! 手机 POST /api/render {lang, source}
//!   → 登记一个等待位（oneshot）
//!   → 发 hub://remote 给桌面端前端（那里有渲染器）
//!   → 桌面端渲染完调 remote_report_render(id, ok, svg)
//!   → 等待位被填上 → HTTP 响应里带上 SVG
//! ```
//!
//! 与 `pdf_screenshot` 同一套路（那边是「前端渲染 PDF 页交回 Rust」），
//! 只是方向相反。**两端都要回话**：失败也要回，否则手机那边要等到超时。
//!
//! 为什么不做缓存：同一段 Mermaid 源码在一条回复里只会出现一次，
//! 而手机刷新页面时聊天记录是从头渲染的——那时候桌面端在渲染队列里排几张图
//! 也就几十毫秒，比维护一份缓存（还要考虑主题切换）便宜。

use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::oneshot;

/// 渲染结果：`ok=false` 时 `payload` 是给人看的错误说明。
#[derive(Debug, Clone)]
pub struct RenderOutcome {
    pub ok: bool,
    pub payload: String,
}

/// 等待位表。一台机器同一时刻只会有几个（一条回复里的几张图）。
#[derive(Default)]
pub struct RenderBridge {
    pending: Mutex<HashMap<String, oneshot::Sender<RenderOutcome>>>,
}

impl RenderBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个等待位。返回的接收端会在桌面端回话（或超时）时完成。
    pub fn begin(&self, id: &str) -> oneshot::Receiver<RenderOutcome> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.to_string(), tx);
        rx
    }

    /// 桌面端回话。返回 false 表示这个 id 已经过期（超时后手机早就放弃了）。
    pub fn finish(&self, id: &str, outcome: RenderOutcome) -> bool {
        match self.pending.lock().remove(id) {
            Some(tx) => tx.send(outcome).is_ok(),
            None => false,
        }
    }

    /// 手机那边断开 / 超时时清掉等待位，别让表一直长。
    pub fn cancel(&self, id: &str) {
        self.pending.lock().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_passes_the_result_back() {
        let bridge = RenderBridge::new();
        let rx = bridge.begin("r1");
        assert!(bridge.finish("r1", RenderOutcome { ok: true, payload: "<svg/>".into() }));
        let got = rx.await.unwrap();
        assert!(got.ok);
        assert_eq!(got.payload, "<svg/>");
        // 同一个 id 不能回两次（第二次说明是迟到的重复回话）
        assert!(!bridge.finish("r1", RenderOutcome { ok: false, payload: "x".into() }));
        // 没登记过的 id 直接忽略
        assert!(!bridge.finish("nope", RenderOutcome { ok: true, payload: String::new() }));
    }

    #[tokio::test]
    async fn cancel_drops_the_slot() {
        let bridge = RenderBridge::new();
        let _rx = bridge.begin("r2");
        bridge.cancel("r2");
        assert!(!bridge.finish("r2", RenderOutcome { ok: true, payload: String::new() }));
    }
}
