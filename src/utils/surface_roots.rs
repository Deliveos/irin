use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// The cached root of every surface, with an index from each root back to its other surfaces.
///
/// The index lets a destroyed surface and the surfaces whose root it was be removed without
/// looking at every surface, which a client can create many of.
#[derive(Debug)]
pub struct SurfaceRoots<S> {
    roots: HashMap<S, S>,
    /// For each root, the other surfaces whose root it is.
    children: HashMap<S, HashSet<S>>,
}

impl<S: Clone + Eq + Hash> SurfaceRoots<S> {
    pub fn new() -> Self {
        Self {
            roots: HashMap::new(),
            children: HashMap::new(),
        }
    }

    pub fn get(&self, surface: &S) -> Option<&S> {
        self.roots.get(surface)
    }

    pub fn insert(&mut self, surface: S, root: S) {
        // This runs on every commit, and the root rarely changes, so check before cloning.
        if self.roots.get(&surface) == Some(&root) {
            return;
        }

        if let Some(old) = self.roots.insert(surface.clone(), root.clone()) {
            if old != surface {
                self.remove_child(&old, &surface);
            }
        }
        if root != surface {
            self.children.entry(root).or_default().insert(surface);
        }
    }

    /// Removes the surface, and the surfaces whose root it is.
    pub fn remove(&mut self, surface: &S) {
        if let Some(root) = self.roots.remove(surface) {
            if root != *surface {
                self.remove_child(&root, surface);
            }
        }

        if let Some(children) = self.children.remove(surface) {
            for child in children {
                self.roots.remove(&child);
            }
        }
    }

    fn remove_child(&mut self, root: &S, child: &S) {
        if let Some(children) = self.children.get_mut(root) {
            children.remove(child);
            if children.is_empty() {
                self.children.remove(root);
            }
        }
    }
}

impl<S: Clone + Eq + Hash> Default for SurfaceRoots<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest_derive::Arbitrary;

    use super::*;

    #[derive(Debug, Clone, Copy, Arbitrary)]
    enum Op {
        Insert {
            #[proptest(strategy = "0..8u8")]
            surface: u8,
            #[proptest(strategy = "0..8u8")]
            root: u8,
        },
        Remove {
            #[proptest(strategy = "0..8u8")]
            surface: u8,
        },
    }

    proptest! {
        #[test]
        fn matches_a_plain_map(ops: Vec<Op>) {
            let mut roots = SurfaceRoots::new();
            // What niri did before: one map, scanned in full on every removal.
            let mut model = HashMap::new();

            for op in ops {
                match op {
                    Op::Insert { surface, root } => {
                        roots.insert(surface, root);
                        model.insert(surface, root);
                    }
                    Op::Remove { surface } => {
                        roots.remove(&surface);
                        model.retain(|k, v| *k != surface && *v != surface);
                    }
                }

                prop_assert_eq!(&roots.roots, &model);

                // The index has exactly the surfaces whose root is another surface.
                let mut children: HashMap<u8, HashSet<u8>> = HashMap::new();
                for (surface, root) in &model {
                    if surface != root {
                        children.entry(*root).or_default().insert(*surface);
                    }
                }
                prop_assert_eq!(&roots.children, &children);
            }
        }
    }
}
