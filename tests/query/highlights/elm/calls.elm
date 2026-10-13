module Main exposing (..)

answer = String.fromInt 42
--              ^ @function

result = transform 1
--       ^ @function

value = record.field
--             ^ @variable.other.member
