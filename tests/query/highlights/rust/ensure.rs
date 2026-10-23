fn main() {
    anyhow::ensure!(true, "expected {value}");
//                                   ^ @variable
}
