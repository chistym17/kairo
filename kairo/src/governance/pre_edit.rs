use std::collections::HashSet;

/// Model turns allowed after `ready_to_edit` before entering EditOrJustify.
pub const PRE_EDIT_STAGNATION_WINDOW: u32 = 4;

/// Tracks whether enough evidence exists to justify patching.
#[derive(Debug, Default)]
pub struct PreEditTracker {
    pub unique_files_read: HashSet<String>,
    pub has_backend_read: bool,
    pub has_frontend_read: bool,
    pub search_rounds: u32,
    pub plan_diagnosis_complete: bool,
    pub ready_to_edit: bool,
    pub ready_since_turn: Option<u32>,
    pub turns_since_ready: u32,
}

impl PreEditTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe_read(&mut self, path: &str) {
        let norm = normalize_path_key(path);
        self.unique_files_read.insert(norm.clone());
        if is_backend_path(&norm) {
            self.has_backend_read = true;
        }
        if is_frontend_path(&norm) {
            self.has_frontend_read = true;
        }
        self.maybe_set_ready();
    }

    pub fn observe_search(&mut self) {
        self.search_rounds = self.search_rounds.saturating_add(1);
        self.maybe_set_ready();
    }

    pub fn observe_plan(&mut self, steps: &str) {
        let lower = steps.to_ascii_lowercase();
        if steps.contains("[x]")
            && (lower.contains("root cause")
                || lower.contains("identified")
                || lower.contains("diagnos"))
        {
            self.plan_diagnosis_complete = true;
            self.maybe_set_ready();
        }
    }

    pub fn note_edit(&mut self) {
        self.ready_to_edit = false;
        self.ready_since_turn = None;
        self.turns_since_ready = 0;
    }

    pub fn end_turn(&mut self, model_turn: u32, edited_this_turn: bool) {
        if edited_this_turn {
            self.note_edit();
            return;
        }
        if !self.ready_to_edit {
            return;
        }
        if self.ready_since_turn.is_none() {
            self.ready_since_turn = Some(model_turn);
        }
        self.turns_since_ready = self.turns_since_ready.saturating_add(1);
    }

    pub fn should_enter_edit_or_justify(&self) -> bool {
        self.ready_to_edit && self.turns_since_ready >= PRE_EDIT_STAGNATION_WINDOW
    }

    fn maybe_set_ready(&mut self) {
        if self.ready_to_edit {
            return;
        }

        // Cross-stack contract mismatch (frontend + backend).
        if self.has_frontend_read && self.has_backend_read && self.unique_files_read.len() >= 2 {
            self.ready_to_edit = true;
            return;
        }

        // Annotation stack: two files under annotate/.
        let annotate_count = self
            .unique_files_read
            .iter()
            .filter(|p| p.contains("annotate/"))
            .count();
        if annotate_count >= 2 {
            self.ready_to_edit = true;
            return;
        }

        // API layer + at least one caller/store file.
        if self
            .unique_files_read
            .iter()
            .any(|p| p.ends_with("api.ts") || p.ends_with("/api.ts"))
            && self.unique_files_read.len() >= 2
        {
            self.ready_to_edit = true;
            return;
        }

        // Search followed by substantive reads.
        if self.search_rounds > 0 && self.unique_files_read.len() >= 2 {
            self.ready_to_edit = true;
            return;
        }

        // Plan marked investigation complete.
        if self.plan_diagnosis_complete && !self.unique_files_read.is_empty() {
            self.ready_to_edit = true;
        }
    }
}

fn normalize_path_key(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn is_backend_path(path: &str) -> bool {
    path.starts_with("backend/") || path.contains("/backend/")
}

fn is_frontend_path(path: &str) -> bool {
    path.starts_with("frontend")
        || path.contains("/frontend")
        || path.starts_with("frontend-v2/")
        || path.contains("/frontend-v2/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_after_frontend_and_backend_reads() {
        let mut t = PreEditTracker::new();
        t.observe_read("frontend-v2/src/lib/api.ts");
        assert!(!t.ready_to_edit);
        t.observe_read("backend/tasks/serializers.py");
        assert!(t.ready_to_edit);
    }

    #[test]
    fn ready_after_annotate_files() {
        let mut t = PreEditTracker::new();
        t.observe_read("frontend-v2/src/components/annotate/polygonDrawing.ts");
        t.observe_read("frontend-v2/src/components/annotate/useAnnotation.ts");
        assert!(t.ready_to_edit);
    }

    #[test]
    fn stagnation_window_after_ready() {
        let mut t = PreEditTracker::new();
        t.observe_read("frontend-v2/src/lib/api.ts");
        t.observe_read("backend/tasks/views.py");
        assert!(t.ready_to_edit);
        for turn in 1..=PRE_EDIT_STAGNATION_WINDOW {
            t.end_turn(turn, false);
            assert_eq!(
                t.should_enter_edit_or_justify(),
                turn >= PRE_EDIT_STAGNATION_WINDOW
            );
        }
    }
}
