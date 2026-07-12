fn main() {
    let specs = std::env::args().nth(1).expect("specs arg");
    oxideav_ac4::asf::debug_region_variant_sweep(&specs);
}
