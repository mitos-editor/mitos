package sample

func formats(value int, writer io.Writer, buffer []byte) {
    fmt.Printf("%d\n", value)
//              ^ @punctuation.special
//               ^ @type
//                ^ @constant.character.escape
    fmt.Sprintf(`%d`, value)
//               ^ @punctuation.special
    fmt.Fprintf(writer, "%d", value, "%s")
//                       ^ @punctuation.special
//                                    ^ @string
    fmt.Appendf(buffer, `%d`, value)
//                       ^ @punctuation.special
    logger.Printf(`%d`, value)
//                 ^ @punctuation.special
    fmt.Println("%d")
//               ^ @string
}
