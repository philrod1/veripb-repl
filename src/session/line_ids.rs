//! What each proof line derived, kept while the lines before it and the
//! formula are unchanged.

/// The constraint IDs proof lines derived, for the lines traced so far.
/// Each holds while the formula and the buffer up to its line are
/// unchanged: [`Self::sync`] drops the ones an edit made stale.
#[derive(Default)]
pub(crate) struct LineIds {
    /// The `FormulaLines::epoch` the IDs hold for.
    epoch: Option<u64>,
    /// Buffer indices, ascending.
    lines: Vec<usize>,
    /// `ids[ends[k - 1]..ends[k]]` are what `lines[k]` derived.
    ends: Vec<usize>,
    ids: Vec<usize>,
}

impl LineIds {
    /// Drops every line's IDs if the formula's epoch is no longer `epoch`,
    /// else those of the lines at or after `changed_from`.
    pub(crate) fn sync(&mut self, epoch: u64, changed_from: Option<usize>) {
        if self.epoch != Some(epoch) {
            *self = LineIds {
                epoch: Some(epoch),
                ..LineIds::default()
            };
        } else if let Some(idx) = changed_from {
            let k = self.lines.partition_point(|&line| line < idx);
            self.lines.truncate(k);
            self.ends.truncate(k);
            self.ids.truncate(self.ends.last().copied().unwrap_or(0));
        }
    }

    /// Returns what buffer line `idx` derived, if it is kept.
    pub(crate) fn get(&self, idx: usize) -> Option<&[usize]> {
        let k = self.lines.binary_search(&idx).ok()?;
        Some(&self.ids[self.start(k)..self.ends[k]])
    }

    pub(crate) fn contains(&self, idx: usize) -> bool {
        self.lines.binary_search(&idx).is_ok()
    }

    fn start(&self, k: usize) -> usize {
        k.checked_sub(1).map_or(0, |prev| self.ends[prev])
    }

    /// Keeps what each of `traced` (buffer index and IDs, in any order)
    /// derived, replacing what was kept for the same lines.
    pub(crate) fn insert(&mut self, traced: impl IntoIterator<Item = (usize, Vec<usize>)>) {
        let mut traced: Vec<(usize, Vec<usize>)> = traced.into_iter().collect();
        if traced.is_empty() {
            return;
        }
        traced.sort_by_key(|(idx, _)| *idx);
        traced.dedup_by_key(|(idx, _)| *idx);
        if self.lines.last().is_none_or(|&last| last < traced[0].0) {
            for (idx, ids) in traced {
                self.push(idx, &ids);
            }
            return;
        }

        let old = std::mem::take(self);
        self.epoch = old.epoch;
        let mut traced = traced.into_iter().peekable();
        for k in 0..old.lines.len() {
            let idx = old.lines[k];
            while let Some((new_idx, ids)) = traced.next_if(|(new_idx, _)| *new_idx < idx) {
                self.push(new_idx, &ids);
            }
            match traced.next_if(|(new_idx, _)| *new_idx == idx) {
                Some((_, ids)) => self.push(idx, &ids),
                None => self.push(idx, &old.ids[old.start(k)..old.ends[k]]),
            }
        }
        for (idx, ids) in traced {
            self.push(idx, &ids);
        }
    }

    fn push(&mut self, idx: usize, ids: &[usize]) {
        self.lines.push(idx);
        self.ids.extend_from_slice(ids);
        self.ends.push(self.ids.len());
    }

    /// Returns the first kept line before buffer index `before` that
    /// derived constraint `id`.
    pub(crate) fn line_of(&self, id: usize, before: usize) -> Option<usize> {
        let end = self.ends[..self.lines.partition_point(|&line| line < before)]
            .last()
            .copied()
            .unwrap_or(0);
        let p = self.ids[..end].iter().position(|&i| i == id)?;
        Some(self.lines[self.ends.partition_point(|&e| e <= p)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(lines: &[(usize, &[usize])]) -> LineIds {
        let mut ids = LineIds::default();
        ids.sync(0, None);
        ids.insert(lines.iter().map(|(idx, ids)| (*idx, ids.to_vec())));
        ids
    }

    #[test]
    fn lines_are_kept_in_order_whatever_order_they_come_in() {
        let mut ids = store(&[(5, &[10]), (1, &[4, 5])]);
        ids.insert([(3, vec![7]), (5, vec![11]), (9, vec![])]);
        assert_eq!(ids.get(1), Some(&[4, 5][..]));
        assert_eq!(ids.get(3), Some(&[7][..]));
        assert_eq!(ids.get(5), Some(&[11][..]), "replaced");
        assert_eq!(ids.get(9), Some(&[][..]));
        assert_eq!(ids.get(2), None);
        assert!(ids.contains(9) && !ids.contains(4));
    }

    #[test]
    fn an_edit_drops_the_lines_from_it_on() {
        let mut ids = store(&[(1, &[4]), (3, &[5]), (5, &[6])]);
        ids.sync(0, Some(3));
        assert!(ids.contains(1) && !ids.contains(3) && !ids.contains(5));
        ids.insert([(4, vec![9])]);
        assert_eq!(ids.get(4), Some(&[9][..]));

        ids.sync(1, None);
        assert!(!ids.contains(1), "a formula change drops every line");
    }

    #[test]
    fn line_of_finds_the_deriving_line_before_a_bound() {
        let ids = store(&[(1, &[4, 5]), (3, &[]), (4, &[6])]);
        assert_eq!(ids.line_of(5, 10), Some(1));
        assert_eq!(ids.line_of(6, 10), Some(4));
        assert_eq!(ids.line_of(6, 4), None);
        assert_eq!(ids.line_of(7, 10), None);
    }
}
