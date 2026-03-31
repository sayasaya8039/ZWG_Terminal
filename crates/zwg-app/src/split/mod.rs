//! Split pane tree — manages horizontal/vertical terminal splits

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::*;
use smallvec::SmallVec;
use uuid::Uuid;

use crate::terminal::{TerminalPane, TerminalSettings};

/// Monotonic pane ID counter for tmux-compatible pane addressing
static NEXT_PANE_ID: AtomicU32 = AtomicU32::new(0);

/// Allocate the next unique pane ID
pub fn next_pane_id() -> u32 {
    NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed)
}

const PANE_BORDER_NEUTRAL: u32 = 0x3C3C3E;

/// Direction of a split
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitDirection {
    Horizontal, // side by side (left | right)
    Vertical,   // stacked (top / bottom)
}

/// Node in the split tree
enum SplitNode {
    Leaf {
        id: Uuid,
        pane_id: u32,
        terminal: Entity<TerminalPane>,
    },
    Branch {
        direction: SplitDirection,
        ratio: f32, // 0.0..1.0, fraction for first child
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

/// Split pane container — manages a tree of terminal panes
pub struct SplitContainer {
    root: SplitNode,
    focused_id: Uuid,
    shell: String,
    terminal_settings: TerminalSettings,
    resize_drag: Option<ResizeDragState>,
    /// When true, only the focused leaf is rendered (zoom mode)
    maximized: bool,
}

/// SmallVec with inline capacity 8 — typical split depth rarely exceeds 8
type BranchPath = SmallVec<[bool; 8]>;

struct ResizeDragState {
    branch_path: BranchPath,
    direction: SplitDirection,
    last_position: Point<Pixels>,
}

impl SplitContainer {
    pub fn new(shell: &str, terminal_settings: TerminalSettings, cx: &mut Context<Self>) -> Self {
        let id = Uuid::new_v4();
        let pane_id = next_pane_id();
        let terminal = cx.new(|cx| TerminalPane::new(shell, terminal_settings.clone(), cx));
        Self {
            root: SplitNode::Leaf { id, pane_id, terminal },
            focused_id: id,
            shell: shell.to_string(),
            terminal_settings,
            resize_drag: None,
            maximized: false,
        }
    }

    /// Split the focused pane in the given direction
    pub fn split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        let target_id = self.focused_id;
        let shell = self.shell.clone();
        let new_id = Uuid::new_v4();
        let new_pane_id = next_pane_id();
        let terminal_settings = self.terminal_settings.clone();
        let new_terminal = cx.new(|cx| TerminalPane::new(&shell, terminal_settings, cx));

        self.root = Self::split_node(
            std::mem::replace(
                &mut self.root,
                SplitNode::Leaf {
                    id: Uuid::nil(),
                    pane_id: u32::MAX,
                    terminal: new_terminal.clone(),
                },
            ),
            target_id,
            direction,
            new_id,
            new_pane_id,
            new_terminal,
        );

        self.focused_id = new_id;
        cx.notify();
    }

    /// Get the pane_id of the most recently focused pane
    pub fn focused_pane_id(&self) -> Option<u32> {
        Self::find_pane_id_by_uuid(&self.root, self.focused_id)
    }

    fn find_pane_id_by_uuid(node: &SplitNode, target_uuid: Uuid) -> Option<u32> {
        match node {
            SplitNode::Leaf { id, pane_id, .. } if *id == target_uuid => Some(*pane_id),
            SplitNode::Leaf { .. } => None,
            SplitNode::Branch { first, second, .. } => {
                Self::find_pane_id_by_uuid(first, target_uuid)
                    .or_else(|| Self::find_pane_id_by_uuid(second, target_uuid))
            }
        }
    }

    fn split_node(
        node: SplitNode,
        target_id: Uuid,
        direction: SplitDirection,
        new_id: Uuid,
        new_pane_id: u32,
        new_terminal: Entity<TerminalPane>,
    ) -> SplitNode {
        match node {
            SplitNode::Leaf { id, pane_id, terminal } if id == target_id => SplitNode::Branch {
                direction,
                ratio: 0.5,
                first: Box::new(SplitNode::Leaf { id, pane_id, terminal }),
                second: Box::new(SplitNode::Leaf {
                    id: new_id,
                    pane_id: new_pane_id,
                    terminal: new_terminal,
                }),
            },
            SplitNode::Branch {
                direction: d,
                ratio,
                first,
                second,
            } => SplitNode::Branch {
                direction: d,
                ratio,
                first: Box::new(Self::split_node(
                    *first,
                    target_id,
                    direction,
                    new_id,
                    new_pane_id,
                    new_terminal.clone(),
                )),
                second: Box::new(Self::split_node(
                    *second,
                    target_id,
                    direction,
                    new_id,
                    new_pane_id,
                    new_terminal,
                )),
            },
            other => other,
        }
    }

    /// Get the first terminal Entity (cheap clone, no PTY spawn)
    fn first_terminal(&self) -> Entity<TerminalPane> {
        Self::get_first_terminal(&self.root)
    }

    fn get_first_terminal(node: &SplitNode) -> Entity<TerminalPane> {
        match node {
            SplitNode::Leaf { terminal, .. } => terminal.clone(),
            SplitNode::Branch { first, .. } => Self::get_first_terminal(first),
        }
    }

    /// Close the focused pane. Returns false if it's the last pane.
    /// Toggle maximize (zoom) for the focused pane
    pub fn toggle_maximize(&mut self, cx: &mut Context<Self>) {
        self.maximized = !self.maximized;
        cx.notify();
    }

    /// Find a terminal entity by leaf UUID
    fn find_terminal_by_id(&self, id: Uuid) -> Option<&Entity<TerminalPane>> {
        Self::find_terminal_in_node(&self.root, id)
    }

    fn find_terminal_in_node(node: &SplitNode, id: Uuid) -> Option<&Entity<TerminalPane>> {
        match node {
            SplitNode::Leaf {
                id: leaf_id,
                terminal,
                ..
            } => {
                if *leaf_id == id {
                    Some(terminal)
                } else {
                    None
                }
            }
            SplitNode::Branch { first, second, .. } => {
                Self::find_terminal_in_node(first, id)
                    .or_else(|| Self::find_terminal_in_node(second, id))
            }
        }
    }

    pub fn close_focused(&mut self, cx: &mut Context<Self>) -> bool {
        let target_id = self.focused_id;

        if let SplitNode::Leaf { id, .. } = &self.root {
            if *id == target_id {
                return false; // last pane, can't close
            }
        }

        // Use existing terminal clone as dummy (no new PTY spawned)
        let dummy_terminal = self.first_terminal();
        let (new_root, sibling_id) = Self::remove_node(
            std::mem::replace(
                &mut self.root,
                SplitNode::Leaf {
                    id: Uuid::nil(),
                    pane_id: u32::MAX,
                    terminal: dummy_terminal,
                },
            ),
            target_id,
        );

        if let Some(root) = new_root {
            self.root = root;
            if let Some(sid) = sibling_id {
                self.focused_id = sid;
            }
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Remove a node from the tree, returning the new tree and the sibling's first leaf ID
    fn remove_node(node: SplitNode, target_id: Uuid) -> (Option<SplitNode>, Option<Uuid>) {
        match node {
            SplitNode::Branch {
                direction,
                ratio,
                first,
                second,
            } => {
                // Check if first child is the target
                if let SplitNode::Leaf { id, .. } = first.as_ref() {
                    if *id == target_id {
                        let sid = Self::first_leaf_id(&second);
                        return (Some(*second), Some(sid));
                    }
                }
                // Check if second child is the target
                if let SplitNode::Leaf { id, .. } = second.as_ref() {
                    if *id == target_id {
                        let sid = Self::first_leaf_id(&first);
                        return (Some(*first), Some(sid));
                    }
                }
                // Recurse into children — preserve original direction and ratio
                let (new_first, sid1) = Self::remove_node(*first, target_id);
                if let Some(nf) = new_first {
                    if sid1.is_some() {
                        return (
                            Some(SplitNode::Branch {
                                direction,
                                ratio,
                                first: Box::new(nf),
                                second,
                            }),
                            sid1,
                        );
                    }
                    let (new_second, sid2) = Self::remove_node(*second, target_id);
                    if let Some(ns) = new_second {
                        return (
                            Some(SplitNode::Branch {
                                direction,
                                ratio,
                                first: Box::new(nf),
                                second: Box::new(ns),
                            }),
                            sid2,
                        );
                    }
                }
                (None, None)
            }
            other => (Some(other), None),
        }
    }

    fn first_leaf_id(node: &SplitNode) -> Uuid {
        match node {
            SplitNode::Leaf { id, .. } => *id,
            SplitNode::Branch { first, .. } => Self::first_leaf_id(first),
        }
    }

    /// Get all terminal entities in order (uuid, terminal)
    pub fn all_terminals(&self) -> Vec<(Uuid, Entity<TerminalPane>)> {
        let mut result = Vec::new();
        Self::collect_terminals(&self.root, &mut result);
        result
    }

    pub fn focused_terminal(&self) -> Option<Entity<TerminalPane>> {
        Self::find_terminal(&self.root, self.focused_id)
    }

    fn collect_terminals(node: &SplitNode, out: &mut Vec<(Uuid, Entity<TerminalPane>)>) {
        match node {
            SplitNode::Leaf { id, terminal, .. } => out.push((*id, terminal.clone())),
            SplitNode::Branch { first, second, .. } => {
                Self::collect_terminals(first, out);
                Self::collect_terminals(second, out);
            }
        }
    }

    fn find_terminal(node: &SplitNode, target_id: Uuid) -> Option<Entity<TerminalPane>> {
        match node {
            SplitNode::Leaf { id, terminal, .. } if *id == target_id => Some(terminal.clone()),
            SplitNode::Leaf { .. } => None,
            SplitNode::Branch { first, second, .. } => Self::find_terminal(first, target_id)
                .or_else(|| Self::find_terminal(second, target_id)),
        }
    }

    // ── IPC pane operations ─────────────────────────────────────────

    /// List all panes with (pane_id, uuid, terminal_entity)
    pub fn list_panes(&self) -> Vec<(u32, Uuid, Entity<TerminalPane>)> {
        let mut result = Vec::new();
        Self::collect_panes(&self.root, &mut result);
        result
    }

    fn collect_panes(node: &SplitNode, out: &mut Vec<(u32, Uuid, Entity<TerminalPane>)>) {
        match node {
            SplitNode::Leaf { id, pane_id, terminal } => {
                out.push((*pane_id, *id, terminal.clone()));
            }
            SplitNode::Branch { first, second, .. } => {
                Self::collect_panes(first, out);
                Self::collect_panes(second, out);
            }
        }
    }

    /// Find a terminal by its numeric pane ID
    pub fn find_pane_by_id(&self, target_pane_id: u32) -> Option<Entity<TerminalPane>> {
        Self::find_pane_by_numeric_id(&self.root, target_pane_id)
    }

    fn find_pane_by_numeric_id(node: &SplitNode, target: u32) -> Option<Entity<TerminalPane>> {
        match node {
            SplitNode::Leaf { pane_id, terminal, .. } if *pane_id == target => {
                Some(terminal.clone())
            }
            SplitNode::Leaf { .. } => None,
            SplitNode::Branch { first, second, .. } => {
                Self::find_pane_by_numeric_id(first, target)
                    .or_else(|| Self::find_pane_by_numeric_id(second, target))
            }
        }
    }

    /// Focus a pane by its numeric pane ID. Returns true if found.
    pub fn focus_pane_by_id(&mut self, target_pane_id: u32, cx: &mut Context<Self>) -> bool {
        if let Some(uuid) = Self::find_uuid_by_pane_id(&self.root, target_pane_id) {
            self.focused_id = uuid;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn find_uuid_by_pane_id(node: &SplitNode, target: u32) -> Option<Uuid> {
        match node {
            SplitNode::Leaf { id, pane_id, .. } if *pane_id == target => Some(*id),
            SplitNode::Leaf { .. } => None,
            SplitNode::Branch { first, second, .. } => {
                Self::find_uuid_by_pane_id(first, target)
                    .or_else(|| Self::find_uuid_by_pane_id(second, target))
            }
        }
    }

    /// Kill (remove) a pane by its numeric ID. Returns true if removed.
    pub fn kill_pane_by_id(&mut self, target_pane_id: u32, cx: &mut Context<Self>) -> bool {
        // Find the UUID for this pane_id
        let Some(target_uuid) = Self::find_uuid_by_pane_id(&self.root, target_pane_id) else {
            return false;
        };

        // Cannot kill the last pane
        if let SplitNode::Leaf { id, .. } = &self.root {
            if *id == target_uuid {
                return false;
            }
        }

        let dummy_terminal = self.first_terminal();
        let (new_root, sibling_id) = Self::remove_node(
            std::mem::replace(
                &mut self.root,
                SplitNode::Leaf {
                    id: Uuid::nil(),
                    pane_id: u32::MAX,
                    terminal: dummy_terminal,
                },
            ),
            target_uuid,
        );

        if let Some(root) = new_root {
            self.root = root;
            if let Some(sid) = sibling_id {
                self.focused_id = sid;
            }
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Count total terminal leaves without allocating a Vec
    fn terminal_count(node: &SplitNode) -> usize {
        match node {
            SplitNode::Leaf { .. } => 1,
            SplitNode::Branch { first, second, .. } => {
                Self::terminal_count(first) + Self::terminal_count(second)
            }
        }
    }

    /// Focus the next pane in the given direction
    pub fn focus_direction(&mut self, dir: FocusDir, cx: &mut Context<Self>) {
        // Quick check without allocating
        if Self::terminal_count(&self.root) <= 1 {
            return;
        }
        let terminals = self.all_terminals();
        let current_idx = terminals
            .iter()
            .position(|(id, _)| *id == self.focused_id)
            .unwrap_or(0);

        let new_idx = match dir {
            FocusDir::Next => (current_idx + 1) % terminals.len(),
            FocusDir::Prev => {
                if current_idx == 0 {
                    terminals.len() - 1
                } else {
                    current_idx - 1
                }
            }
        };

        self.focused_id = terminals[new_idx].0;
        cx.notify();
    }

    pub fn update_terminal_settings(
        &mut self,
        terminal_settings: TerminalSettings,
        cx: &mut Context<Self>,
    ) {
        self.terminal_settings = terminal_settings.clone();
        for (_, terminal) in self.all_terminals() {
            let terminal_settings = terminal_settings.clone();
            terminal.update(cx, |pane, cx| {
                pane.update_settings(&terminal_settings, cx);
            });
        }
    }

    fn begin_resize_drag(
        &mut self,
        branch_path: BranchPath,
        direction: SplitDirection,
        start: Point<Pixels>,
    ) {
        self.resize_drag = Some(ResizeDragState {
            branch_path,
            direction,
            last_position: start,
        });
    }

    fn end_resize_drag(&mut self) {
        self.resize_drag = None;
    }

    fn update_resize_drag(
        &mut self,
        position: Point<Pixels>,
        viewport_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        const MIN_RATIO: f32 = 0.15;
        const MAX_RATIO: f32 = 0.85;

        let Some(drag) = self.resize_drag.as_mut() else {
            return;
        };

        let one = px(1.0);
        let delta_px = match drag.direction {
            SplitDirection::Horizontal => (position.x - drag.last_position.x) / one,
            SplitDirection::Vertical => (position.y - drag.last_position.y) / one,
        };
        if delta_px.abs() < 0.01 {
            return;
        }
        drag.last_position = position;

        let axis_span = match drag.direction {
            SplitDirection::Horizontal => (viewport_size.width / one).max(1.0),
            SplitDirection::Vertical => (viewport_size.height / one).max(1.0),
        };
        let delta_ratio = delta_px / axis_span;

        if Self::adjust_branch_ratio_by_path(
            &mut self.root,
            &drag.branch_path,
            delta_ratio,
            MIN_RATIO,
            MAX_RATIO,
        ) {
            cx.notify();
        }
    }

    fn adjust_branch_ratio_by_path(
        node: &mut SplitNode,
        path: &[bool],
        delta_ratio: f32,
        min_ratio: f32,
        max_ratio: f32,
    ) -> bool {
        if path.is_empty() {
            if let SplitNode::Branch { ratio, .. } = node {
                let next = (*ratio + delta_ratio).clamp(min_ratio, max_ratio);
                if (next - *ratio).abs() > f32::EPSILON {
                    *ratio = next;
                    return true;
                }
            }
            return false;
        }

        match node {
            SplitNode::Branch { first, second, .. } => {
                if path[0] {
                    Self::adjust_branch_ratio_by_path(
                        second,
                        &path[1..],
                        delta_ratio,
                        min_ratio,
                        max_ratio,
                    )
                } else {
                    Self::adjust_branch_ratio_by_path(
                        first,
                        &path[1..],
                        delta_ratio,
                        min_ratio,
                        max_ratio,
                    )
                }
            }
            SplitNode::Leaf { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FocusDir {
    Next,
    Prev,
}

fn leaf_border_style(is_focused: bool) -> (u8, u32) {
    let width = if is_focused { 1 } else { 1 };
    (width, PANE_BORDER_NEUTRAL)
}

impl Render for SplitContainer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused_id = self.focused_id;

        // In maximized mode, render only the focused leaf
        if self.maximized {
            if let Some(terminal) = self.find_terminal_by_id(focused_id) {
                return div()
                    .size_full()
                    .child(terminal.clone())
                    .on_mouse_move(cx.listener(|_, _, _, _| {}))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|_, _, _, _| {}),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|_, _, _, _| {}),
                    );
            }
        }

        let empty_path: BranchPath = SmallVec::new();
        Self::render_node(&self.root, focused_id, cx.entity().clone(), &empty_path)
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if ev.dragging() {
                    this.update_resize_drag(ev.position, window.viewport_size(), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.end_resize_drag();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.end_resize_drag();
                }),
            )
    }
}

impl SplitContainer {
    fn render_node(
        node: &SplitNode,
        focused_id: Uuid,
        pane_entity: Entity<SplitContainer>,
        branch_path: &BranchPath,
    ) -> Div {
        match node {
            SplitNode::Leaf { id, terminal, .. } => {
                let is_focused = *id == focused_id;
                let (border_width, border_rgb) = leaf_border_style(is_focused);
                let mut el = div().size_full().child(terminal.clone());
                el = match border_width {
                    0 => el,
                    1 => el.border_1(),
                    2 => el.border_2(),
                    _ => el.border_1(),
                };
                el = el.border_color(rgb(border_rgb));
                el
            }
            SplitNode::Branch {
                direction,
                ratio,
                first,
                second,
            } => {
                let mut first_path: BranchPath = branch_path.clone();
                first_path.push(false);
                let first_el =
                    Self::render_node(first, focused_id, pane_entity.clone(), &first_path);
                let mut second_path: BranchPath = branch_path.clone();
                second_path.push(true);
                let second_el =
                    Self::render_node(second, focused_id, pane_entity.clone(), &second_path);
                let r = *ratio;

                match direction {
                    SplitDirection::Horizontal => div()
                        .size_full()
                        .flex()
                        .flex_row()
                        .child(
                            first_el
                                .flex_grow()
                                .flex_basis(relative(r))
                                .min_w(px(140.0)),
                        )
                        .child({
                            let pane_resize = pane_entity.clone();
                            let path_for_start: BranchPath = branch_path.clone();
                            div()
                                .w(px(4.0))
                                .h_full()
                                .cursor_col_resize()
                                .on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                                    pane_resize.update(cx, |pane, _cx| {
                                        pane.begin_resize_drag(
                                            path_for_start.clone(),
                                            SplitDirection::Horizontal,
                                            ev.position,
                                        );
                                    });
                                })
                                .child(div().mx(px(1.5)).w(px(1.0)).h_full().bg(rgb(0x3C3C3E)))
                        })
                        .child(
                            second_el
                                .flex_grow()
                                .flex_basis(relative(1.0 - r))
                                .min_w(px(140.0)),
                        ),
                    SplitDirection::Vertical => div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(first_el.flex_grow().flex_basis(relative(r)).min_h(px(96.0)))
                        .child({
                            let pane_resize = pane_entity.clone();
                            let path_for_start: BranchPath = branch_path.clone();
                            div()
                                .h(px(4.0))
                                .w_full()
                                .cursor_row_resize()
                                .on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                                    pane_resize.update(cx, |pane, _cx| {
                                        pane.begin_resize_drag(
                                            path_for_start.clone(),
                                            SplitDirection::Vertical,
                                            ev.position,
                                        );
                                    });
                                })
                                .child(div().my(px(1.5)).h(px(1.0)).w_full().bg(rgb(0x3C3C3E)))
                        })
                        .child(
                            second_el
                                .flex_grow()
                                .flex_basis(relative(1.0 - r))
                                .min_h(px(96.0)),
                        ),
                }
            }
        }
    }
}

impl EventEmitter<()> for SplitContainer {}

#[cfg(test)]
mod tests {
    use super::{PANE_BORDER_NEUTRAL, leaf_border_style};

    #[test]
    fn leaf_border_style_avoids_blue_focus_ring() {
        assert_eq!(leaf_border_style(true), (1, PANE_BORDER_NEUTRAL));
        assert_eq!(leaf_border_style(false), (1, PANE_BORDER_NEUTRAL));
    }
}
