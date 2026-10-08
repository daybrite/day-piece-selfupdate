fn main() {
    if dayapp::fixture_command() {
        return;
    }
    day::launch(dayapp::window(), dayapp::root);
}
