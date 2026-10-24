use std::collections::VecDeque;

/// Fixed-length series of samples, oldest first.
#[derive(Clone, Default, Debug)]
pub struct History {
    values: VecDeque<f32>,
    cap: usize,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self { values: VecDeque::with_capacity(cap), cap }
    }

    pub fn push(&mut self, v: f32) {
        if self.cap == 0 {
            return;
        }
        if self.values.len() == self.cap {
            self.values.pop_front();
        }
        self.values.push_back(v);
    }

    pub fn last(&self) -> Option<f32> {
        self.values.back().copied()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn get(&self, i: usize) -> Option<f32> {
        self.values.get(i).copied()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = f32> + ExactSizeIterator + '_ {
        self.values.iter().copied()
    }

    /// Largest finite value, if any.
    pub fn max(&self) -> Option<f32> {
        self.iter().filter(|v| v.is_finite()).reduce(f32::max)
    }

    pub fn min(&self) -> Option<f32> {
        self.iter().filter(|v| v.is_finite()).reduce(f32::min)
    }
}
