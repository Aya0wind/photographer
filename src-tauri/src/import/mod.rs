//! 导入引擎（spec §5.2）：M1 T5/T7 实现（模板、导入流水线、分层查重、断点恢复）。
//! M2：F1 安全清卡（clean）、F2 双目的地（engine::SecondTarget）。

pub mod clean;
pub mod engine;
pub mod templates;
