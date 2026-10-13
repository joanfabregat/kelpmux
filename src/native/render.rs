use super::{protocol::*, workspace::Workspace};
use std::collections::BTreeMap;
pub fn compose(
    workspace: &Workspace,
    _screens: &BTreeMap<PaneId, Screen>,
    controller: bool,
    notice: Option<&str>,
    _spinner: usize,
) -> Frame {
    Frame {
        cols: workspace.cols,
        rows: workspace.rows,
        cells: vec![],
        cursor: None,
        panes: workspace.layout(),
        active: workspace.active,
        reading: workspace.reading,
        controller,
        help: workspace.help,
        notice: notice.map(str::to_owned),
    }
}
pub fn terminal_diff(_previous: Option<&Frame>, _frame: &Frame) -> Vec<u8> {
    vec![]
}
