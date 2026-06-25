//! Typed inventory for the AI explorer.
//!
//! Unlike [`BagContent`](crate::explorer::BagContent), which only stores counts,
//! the [`Bag`] keeps the real resource objects. They are needed to build a
//! [`ComplexResourceRequest`](common_game::components::resource::ComplexResourceRequest)
//! when crafting: the planet wants the actual ingredients, not just their number.

use std::collections::HashMap;

use common_game::components::resource::{
    BasicResource, BasicResourceType, ComplexResource, ComplexResourceType, ResourceType,
};

use crate::explorer::BagContent;

/// The explorer's inventory, grouped by resource type.
#[derive(Default)]
pub struct Bag {
    basics: HashMap<BasicResourceType, Vec<BasicResource>>,
    complex: HashMap<ComplexResourceType, Vec<ComplexResource>>,
}

impl Bag {
    /// Stores a basic resource.
    pub fn add_basic(&mut self, resource: BasicResource) {
        self.basics.entry(resource.get_type()).or_default().push(resource);
    }

    /// Stores a complex resource.
    pub fn add_complex(&mut self, resource: ComplexResource) {
        self.complex.entry(resource.get_type()).or_default().push(resource);
    }

    /// Removes and returns one basic resource of the given type, if present.
    pub fn take_basic(&mut self, ty: BasicResourceType) -> Option<BasicResource> {
        self.basics.get_mut(&ty)?.pop()
    }

    /// Removes and returns one complex resource of the given type, if present.
    pub fn take_complex(&mut self, ty: ComplexResourceType) -> Option<ComplexResource> {
        self.complex.get_mut(&ty)?.pop()
    }

    /// How many basic resources of `ty` are in the bag.
    pub fn count_basic(&self, ty: BasicResourceType) -> usize {
        self.basics.get(&ty).map_or(0, Vec::len)
    }

    /// How many complex resources of `ty` are in the bag.
    pub fn count_complex(&self, ty: ComplexResourceType) -> usize {
        self.complex.get(&ty).map_or(0, Vec::len)
    }

    /// Empties the bag.
    pub fn clear(&mut self) {
        self.basics.clear();
        self.complex.clear();
    }

    /// Produces the count-only [`BagContent`] view sent back to the orchestrator.
    pub fn to_content(&self) -> BagContent {
        let mut content = BagContent::default();
        for (&ty, items) in &self.basics {
            if !items.is_empty() {
                content.content.insert(ResourceType::Basic(ty), items.len());
            }
        }
        for (&ty, items) in &self.complex {
            if !items.is_empty() {
                content.content.insert(ResourceType::Complex(ty), items.len());
            }
        }
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carbon() -> BasicResource {
        BasicResource::Carbon(unsafe { std::mem::zeroed() })
    }

    #[test]
    fn add_count_and_take_round_trip() {
        let mut bag = Bag::default();
        assert_eq!(bag.count_basic(BasicResourceType::Carbon), 0);
        bag.add_basic(carbon());
        bag.add_basic(carbon());
        assert_eq!(bag.count_basic(BasicResourceType::Carbon), 2);

        assert!(bag.take_basic(BasicResourceType::Carbon).is_some());
        assert_eq!(bag.count_basic(BasicResourceType::Carbon), 1);

        assert!(bag.take_basic(BasicResourceType::Oxygen).is_none());
    }

    #[test]
    fn to_content_reports_counts_and_skips_empties() {
        let mut bag = Bag::default();
        bag.add_basic(carbon());
        bag.add_basic(carbon());
        let content = bag.to_content();
        assert_eq!(
            content.content.get(&ResourceType::Basic(BasicResourceType::Carbon)),
            Some(&2)
        );
        assert!(
            !content
                .content
                .contains_key(&ResourceType::Basic(BasicResourceType::Oxygen))
        );
    }
}
