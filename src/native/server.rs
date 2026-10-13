use crate::Result;
use std::path::PathBuf;
pub struct ServerConfig {
    pub socket: PathBuf,
    pub state_dir: PathBuf,
    pub shell: PathBuf,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}
pub fn serve(_config: ServerConfig) -> Result<()> {
    Err("server implementation pending".into())
}
