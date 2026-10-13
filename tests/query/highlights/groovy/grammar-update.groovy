def demo() {
  outer: while (true) {
//^ @label
    break outer
  }
}
