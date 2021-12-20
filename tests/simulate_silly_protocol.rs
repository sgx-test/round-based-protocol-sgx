use round_based::dev::Simulation;

use crate::silly_protocol::MultiPartyGenRandom;

mod silly_protocol;

#[test]
fn simulate_silly_protocol() {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    let mut rnd = rand::thread_rng();
    let mut simulation = Simulation::new();
    simulation
        .enable_benchmarks(true)
        .add_party(MultiPartyGenRandom::with_fixed_seed(1, 3, 10, &mut rnd))
        .add_party(MultiPartyGenRandom::with_fixed_seed(2, 3, 20, &mut rnd))
        .add_party(MultiPartyGenRandom::with_fixed_seed(3, 3, 30, &mut rnd));
    let result = simulation.run().expect("simulation failed");
    assert_eq!(result, vec![10 ^ 20 ^ 30; 3]);
    println!("Benchmarks:");
    println!("{:#?}", simulation.benchmark_results().unwrap());
}
