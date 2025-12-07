use common_game::components::{
    energy_cell::EnergyCell,
    planet::PlanetState,
    resource::{
        BasicResource, BasicResourceType, Combinator, ComplexResource, ComplexResourceRequest,
        Generator, GenericResource,
    },
};

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

    let result = match resource {
        Carbon => generator.make_carbon(cell).map(BasicResource::Carbon),
        Hydrogen => generator.make_hydrogen(cell).map(BasicResource::Hydrogen),
        Oxygen => generator.make_oxygen(cell).map(BasicResource::Oxygen),
        Silicon => generator.make_silicon(cell).map(BasicResource::Silicon),
    };

    result.ok()
}

pub fn combine_complex(
    combinator: &Combinator,
    cell: &mut EnergyCell,
    req: ComplexResourceRequest,
) -> Result<ComplexResource, (String, GenericResource, GenericResource)> {
    use ComplexResourceRequest::*;

    match req {
        Water(h, o) => combinator
            .make_water(h, o, cell)
            .map(|w| w.to_complex())
            .map_err(|(e, h, o)| (e, h.to_generic(), o.to_generic())),

        AIPartner(r, d) => combinator
            .make_aipartner(r, d, cell)
            .map(|a| a.to_complex())
            .map_err(|(e, r, d)| (e, r.to_generic(), d.to_generic())),

        Diamond(c1, c2) => combinator
            .make_diamond(c1, c2, cell)
            .map(|d| d.to_complex())
            .map_err(|(e, c1, c2)| (e, c1.to_generic(), c2.to_generic())),

        Dolphin(w, l) => combinator
            .make_dolphin(w, l, cell)
            .map(|d| d.to_complex())
            .map_err(|(e, w, l)| (e, w.to_generic(), l.to_generic())),

        Life(w, c) => combinator
            .make_life(w, c, cell)
            .map(|l| l.to_complex())
            .map_err(|(e, w, c)| (e, w.to_generic(), c.to_generic())),

        Robot(s, l) => combinator
            .make_robot(s, l, cell)
            .map(|r| r.to_complex())
            .map_err(|(e, s, l)| (e, s.to_generic(), l.to_generic())),
    }
}
