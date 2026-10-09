//! Undo and redo as whole-project snapshots. A project is a few kilobytes of
//! JSON-shaped data, so cloning it per edit is cheaper than inverting edits.

use crate::model::Project;

const LIMIT: usize = 500;

#[derive(Default)]
pub struct History {
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl History {
    /// Records the state from before an edit.
    pub fn record(&mut self, before: Project) {
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &mut Project) -> bool {
        match self.undo.pop() {
            Some(previous) => {
                self.redo.push(std::mem::replace(current, previous));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, current: &mut Project) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(current, next));
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo() {
        let mut history = History::default();
        let mut project = Project::default();
        let before = project.clone();
        project.settings.fps = 24;
        history.record(before);
        assert!(history.undo(&mut project));
        assert_eq!(project.settings.fps, 30);
        assert!(history.redo(&mut project));
        assert_eq!(project.settings.fps, 24);
        assert!(!history.redo(&mut project));
    }
}
