//! 网络层基础设施
//!
//! 提供全局 HTTP 客户端工厂，统一管理出站代理配置与热更新。

pub mod http_client;

pub use http_client::HttpClientFactory;
