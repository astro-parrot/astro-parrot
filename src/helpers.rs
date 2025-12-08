use common_game::components::{
    energy_cell::EnergyCell,
    planet::PlanetState,
    resource::{
        BasicResource, BasicResourceType, Combinator, ComplexResource, ComplexResourceRequest,
        Generator, GenericResource,
    },
};

macro_rules! simple_comb {
    ($resource:expr, $gen:expr, $cell:expr, { $($variant:ident,)* }) => {
        match $resource {
            $(
                $variant => {
                    paste::paste! {
                        $gen.[<make_ $variant:lower>]($cell)
                            .map(BasicResource::$variant)
                    }
                },
            )*
        }
    };
}

macro_rules! complex_comb {
    ($req:expr, $comb:expr, $cell:expr, { $($variant:ident,)* }) => {
        match $req {
            $(
                $variant(a, b) => {
                    paste::paste! {
                        $comb.[<make_ $variant:lower>](a, b, $cell)
                            .map(|v| v.to_complex())
                            .map_err(|(e, a, b)| (e, a.to_generic(), b.to_generic()))
                    }
                },
            )*
        }
    };
}

pub fn try_build_rocket(state: &mut PlanetState) {
    if let Some((_, idx)) = state.full_cell() {
        let _ = state.build_rocket(idx);
    }
}

pub fn generate_basic(
    generator: &Generator,
    cell: &mut EnergyCell,
    resource: BasicResourceType,
) -> Option<BasicResource> {
    use BasicResourceType::*;

    simple_comb!(
        resource,
        generator,
        cell,
        {
            Hydrogen,
            Oxygen,
            Silicon,
            Carbon,
        }
    )
    .ok()
}

pub fn combine_complex(
    combinator: &Combinator,
    cell: &mut EnergyCell,
    req: ComplexResourceRequest,
) -> Result<ComplexResource, (String, GenericResource, GenericResource)> {
    use ComplexResourceRequest::*;

    complex_comb!(
        req,
        combinator,
        cell,
        {
            Water,
            AIPartner,
            Diamond,
            Dolphin,
            Life,
            Robot,
        }
    )
}
