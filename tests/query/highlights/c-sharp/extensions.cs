static class Extensions {
    extension(string text) {
//  ^ @keyword.storage.type
//                   ^ @variable.parameter
        public int Count() => text.Length;
//                 ^ @function.method
    }
}
