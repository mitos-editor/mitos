def f(x) do
  helper(x)
# ^ @function.call
  x |> Mod.doThing()
#          ^ @function.call
  "#{String.upcase(x)}"
#            ^ @function.call
  is_binary(x)
# ^ @function.call
end
