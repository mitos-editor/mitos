fn example() -> Unit {
  let values = [|1, 2|]
//             ^ @punctuation.bracket
//                   ^ @punctuation.bracket
  errdefer cleanup()
//^ @keyword
  lexscan source with longest {
//                    ^ @keyword
    reb"[0-9]+" => ()
//  ^ @string.regexp
  }
}
