function! F(o)
  call G(a:o)
"      ^ @function.call
  let x = a:o.prop
"             ^ @variable.other.member
endfunction
