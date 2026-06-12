//! Flowplane core. In S1 this hosts server configuration; from S2 it becomes the only
//! mutation path (services + authorization engine, spec/10 §2).

pub mod config;

pub use config::ServerConfig;
