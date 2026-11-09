f <- function() {
  helper(1)
# ^ @function.call
  pkg::func(2)
#      ^ @function.call
}
