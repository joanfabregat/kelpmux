//! Browser client assets embedded at build time from `web/dist` (see build.rs).
//! Empty when the executable was built without the web client.
include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
