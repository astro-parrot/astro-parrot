// The crafting tree from common-game, plus the goal the explorer picks. Kept
// separate from the strategy so it can be reasoned about (and tested) on its own.

use std::collections::{HashMap, HashSet};

use common_game::components::resource::{
    BasicResourceType as B, ComplexResourceRequest as Req, ComplexResourceType as C,
};

use super::bag::Bag;

// What the explorer wants, in order of preference: a harem of AI girlfriends,
// then progressively less exciting company.
pub const WISHLIST: [(C, usize); 4] = [
    (C::AIPartner, 10),
    (C::Dolphin, 10),
    (C::Robot, 10),
    (C::Diamond, 25),
];

// The most-wanted resource the galaxy can actually make, or None if it can make
// none of them.
pub fn pick_goal(feasible: &HashSet<C>) -> Option<(C, usize)> {
    WISHLIST.into_iter().find(|(c, _)| feasible.contains(c))
}

// Which complex resources can actually be crafted, given the basics some planet
// generates and the combinations available anywhere. Fixed-point: keep adding
// craftable resources until nothing new becomes reachable, so a craftable
// intermediate can unlock the things that need it.
pub fn feasible_complex(
    available_basics: &HashSet<B>,
    available_combinations: &HashSet<C>,
) -> HashSet<C> {
    let mut craftable: HashSet<C> = HashSet::new();
    loop {
        let mut grew = false;
        for &ty in available_combinations {
            if craftable.contains(&ty) {
                continue;
            }
            let (basics, complex) = ingredients(ty);
            let reachable = basics.iter().all(|b| available_basics.contains(b))
                && complex.iter().all(|c| craftable.contains(c));
            if reachable {
                craftable.insert(ty);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    craftable
}

// Direct ingredients of a complex resource: (basics, complex).
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

// Depth in the crafting tree, used to craft dependencies before their consumers.
pub fn tier(ty: C) -> u8 {
    match ty {
        C::Diamond | C::Water => 1,
        C::Life => 2,
        C::Robot | C::Dolphin => 3,
        C::AIPartner => 4,
    }
}

// Total basics needed to build `count` of `ty`, recursively.
pub fn basic_cost(ty: C, count: usize, acc: &mut HashMap<B, usize>) {
    let (basics, complex) = ingredients(ty);
    for b in basics {
        *acc.entry(b).or_default() += count;
    }
    for c in complex {
        basic_cost(c, count, acc);
    }
}

// Every complex resource that must be produced to end up with `count` of `ty`,
// intermediates included.
pub fn complex_production(ty: C, count: usize, acc: &mut HashMap<C, usize>) {
    *acc.entry(ty).or_default() += count;
    let (_, complex) = ingredients(ty);
    for c in complex {
        complex_production(c, count, acc);
    }
}

// Takes the ingredients for `ty` out of the bag and builds the request. If the
// second ingredient is missing, the first is put back.
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
        bag.add_basic(carbon());
        assert!(build_request(&mut bag, C::Diamond).is_none());
        assert_eq!(bag.count_basic(B::Carbon), 1);
    }

    #[test]
    fn feasible_excludes_targets_with_missing_basics() {
        // Only Carbon available: AIPartner needs Silicon, so it's out.
        let basics = HashSet::from([B::Carbon]);
        let combos = HashSet::from([C::Diamond, C::AIPartner]);
        let feasible = feasible_complex(&basics, &combos);
        assert!(feasible.contains(&C::Diamond));
        assert!(!feasible.contains(&C::AIPartner));
    }

    #[test]
    fn feasible_unlocks_through_intermediates() {
        let basics = HashSet::from([B::Carbon, B::Hydrogen, B::Oxygen, B::Silicon]);
        let combos =
            HashSet::from([C::Water, C::Diamond, C::Life, C::Robot, C::Dolphin, C::AIPartner]);
        assert_eq!(feasible_complex(&basics, &combos).len(), 6);
    }

    #[test]
    fn feasible_blocks_intermediate_not_combinable_anywhere() {
        // Life needs Water, but no planet combines Water.
        let basics = HashSet::from([B::Carbon, B::Hydrogen, B::Oxygen]);
        let combos = HashSet::from([C::Life]);
        assert!(feasible_complex(&basics, &combos).is_empty());
    }

    #[test]
    fn pick_goal_prefers_a_harem_when_possible() {
        let feasible = HashSet::from([C::AIPartner, C::Dolphin, C::Robot, C::Diamond]);
        assert_eq!(pick_goal(&feasible), Some((C::AIPartner, 10)));
    }

    #[test]
    fn pick_goal_settles_for_the_best_available() {
        let feasible = HashSet::from([C::Dolphin, C::Robot, C::Diamond]);
        assert_eq!(pick_goal(&feasible), Some((C::Dolphin, 10)));
        assert_eq!(pick_goal(&HashSet::from([C::Diamond])), Some((C::Diamond, 25)));
    }

    #[test]
    fn pick_goal_is_none_when_nothing_wanted_is_craftable() {
        assert_eq!(pick_goal(&HashSet::new()), None);
        assert_eq!(pick_goal(&HashSet::from([C::Water])), None);
    }
}
