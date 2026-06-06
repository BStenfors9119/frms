/// The kind of content a pane holds.
///
/// Claude terminals and their permission-prompt response views are no
/// longer separate panes in the pane_grid — they live inside the Editor
/// pane as tabs, so this enum stays small.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaneKind {
    FileBrowser,
    Editor,
    Browser,
}
