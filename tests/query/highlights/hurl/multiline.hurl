GET https://example.com
HTTP 200
```raw
literal\n {{value}}
#      ^ !constant.character.escape
#         ^ !punctuation.special
#           ^ @string
```

GET https://example.com
HTTP 200
```
literal\n {{value}}
#      ^ !constant.character.escape
#         ^ @punctuation.special
#           ^ @variable
```
