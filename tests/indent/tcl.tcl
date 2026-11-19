proc greet {name} {
    if {$name eq "x"} {
        puts "hi"
    } else {
        puts "bye"
    }
}

regsub \
    -all \
    {foo} \
    $input \
    {bar}
