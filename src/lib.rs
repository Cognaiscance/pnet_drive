//! pNet Drive keeps one folder of files for this user.
//!
//! `drive-client` watches a folder on each device. `drive-server` stores the
//! files and mounts `/apps/drive/` on the top-ranked server-grade node.

pub mod log_setup;
pub mod node;
pub mod page;
pub mod store;

pub const CLIENT_ALIAS: &str = "drive-client";
pub const SERVER_ALIAS: &str = "drive-server";
pub const APP_PROTOCOL: &str = "application/drive";
pub const DEFAULT_CLIENT_PUSH: u16 = 8820;
pub const DEFAULT_SERVER_PUSH: u16 = 8821;
pub const DEFAULT_HTTP_PORT: u16 = 8822;
pub const WEB_SLUG: &str = "drive";
pub const WEB_TITLE: &str = "Drive";
