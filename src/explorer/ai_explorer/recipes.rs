//! The static crafting tree shared by every planet in the game.
//!
//! These functions encode the recipes defined by `common-game` so the explorer
//! can reason about *what* it needs (the shopping list of basics, the order in
//! which to craft intermediates) independently of *which* planet can make it.

use std::collections::HashMap;

use common_game::components::resource::{
    BasicResourceType as B, ComplexResourceRequest as Req, ComplexResourceType as C,
};

use super::bag::Bag;

/// The direct ingredients of a complex resource: `(basics, complex)`.
pub fn ingredients(ty: C) -> (Vec<B>, Vec<C>) {
    match ty {
        C::Diamond => (vec![B::Carbon, B::Carbon], vec![]),
        C::Water => (vec![B::Hydrogen, B::Oxygen], vec![]),
        C::Life => (vec![B::Carbon], vec![C::Water]),
        C::Robot => (vec![B::Silicon], vec![C::Life]),
        C::Dolphin => (vec![], vec![C::Water, C::Life]),
        C::AIPartner => (vec![], vec![C::Robot, C::Diamond]),
    }
}

/// Crafting depth, used to order crafts so dependencies are built first.
pub fn tier(ty: C) -> u8 {
    match ty {
        C::Diamond | C::Water => 1,
        C::Life => 2,
        C::Robot | C::Dolphin => 3,
        C::AIPartner => 4,
    }
}

/// Recursively accumulates the basic resources required to build `count` of `ty`.
pub fn basic_cost(ty: C, count: usize, acc: &mut HashMap<B, usize>) {
    let (basics, complex) = ingredients(ty);
    for b in basics {
        *acc.entry(b).or_default() += count;
    }
    for c in complex {
        basic_cost(c, count, acc);
    }
}

/// Recursively accumulates the complex resources that must be *produced* to end
/// up with `count` of `ty` — including every intermediate consumed along the way.
pub fn complex_production(ty: C, count: usize, acc: &mut HashMap<C, usize>) {
    *acc.entry(ty).or_default() += count;
    let (_, complex) = ingredients(ty);
    for c in complex {
        complex_production(c, count, acc);
    }
}

/// Pulls the ingredients for one `ty` out of the bag and assembles the request.
///
/// On a partial match (first ingredient present, second missing) the first one
/// is returned to the bag, so nothing is lost.
pub fn build_request(bag: &mut Bag, ty: C) -> Option<Req> {
    match ty {
        C::Diamond => {
            let a = bag.take_basic(B::Carbon)?;
            let Some(b) = bag.take_basic(B::Carbon) else {
                bag.add_basic(a);
                return None;
            };
            Some(Req::Diamond(a.to_carbon().ok()?, b.to_carbon().ok()?))
        }
        C::Water => {
            let a = bag.take_basic(B::Hydrogen)?;
            let Some(b) = bag.take_basic(B::Oxygen) else {
                bag.add_basic(a);
                return None;
            };
            Some(Req::Water(a.to_hydrogen().ok()?, b.to_oxygen().ok()?))
        }
        C::Life => {
            let a = bag.take_complex(C::Water)?;
            let Some(b) = bag.take_basic(B::Carbon) else {
                bag.add_complex(a);
                return None;
            };
            Some(Req::Life(a.to_water().ok()?, b.to_carbon().ok()?))
        }
        C::Robot => {
            let a = bag.take_basic(B::Silicon)?;
            let Some(b) = bag.take_complex(C::Life) else {
                bag.add_basic(a);
                return None;
            };
            Some(Req::Robot(a.to_silicon().ok()?, b.to_life().ok()?))
        }
        C::Dolphin => {
            let a = bag.take_complex(C::Water)?;
            let Some(b) = bag.take_complex(C::Life) else {
                bag.add_complex(a);
                return None;
            };
            Some(Req::Dolphin(a.to_water().ok()?, b.to_life().ok()?))
        }
        C::AIPartner => {
            let a = bag.take_complex(C::Robot)?;
            let Some(b) = bag.take_complex(C::Diamond) else {
                bag.add_complex(a);
                return None;
            };
            Some(Req::AIPartner(a.to_robot().ok()?, b.to_diamond().ok()?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::bag::Bag;
    use super::*;
    use common_game::components::resource::BasicResource;

    fn carbon() -> BasicResource {
        BasicResource::Carbon(unsafe { std::mem::zeroed() })
    }

    #[test]
    fn tiers_are_ordered_by_dependency_depth() {
        assert!(tier(C::Diamond) < tier(C::Life));
        assert!(tier(C::Life) < tier(C::Robot));
        assert!(tier(C::Robot) < tier(C::AIPartner));
    }

    #[test]
    fn basic_cost_expands_the_whole_tree() {
        let mut acc = HashMap::new();
        basic_cost(C::AIPartner, 1, &mut acc);
        // AIPartner = Robot(Silicon + Life(Water(H+O) + Carbon)) + Diamond(2 Carbon)
        assert_eq!(acc[&B::Silicon], 1);
        assert_eq!(acc[&B::Carbon], 3);
        assert_eq!(acc[&B::Hydrogen], 1);
        assert_eq!(acc[&B::Oxygen], 1);
    }

    #[test]
    fn complex_production_includes_intermediates() {
        let mut acc = HashMap::new();
        complex_production(C::AIPartner, 1, &mut acc);
        for ty in [C::AIPartner, C::Robot, C::Diamond, C::Life, C::Water] {
            assert_eq!(acc.get(&ty).copied().unwrap_or(0), 1, "missing {ty:?}");
        }
    }

    #[test]
    fn build_request_consumes_two_carbon_for_a_diamond() {
        let mut bag = Bag::default();
        bag.add_basic(carbon());
        bag.add_basic(carbon());
        assert!(build_request(&mut bag, C::Diamond).is_some());
        assert_eq!(bag.count_basic(B::Carbon), 0);
    }

    #[test]
    fn build_request_recovers_partial_ingredients() {
        let mut bag = Bag::default();
        bag.add_basic(carbon()); // only one Carbon: not enough for a Diamond
        assert!(build_request(&mut bag, C::Diamond).is_none());
        assert_eq!(bag.count_basic(B::Carbon), 1, "the lone Carbon must be returned");
    }
}
