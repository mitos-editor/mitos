use utf8;

my $café = 42;
#  ^ @variable

sub saluer {
#   ^ @function
  return $café;
#        ^ @variable
}

my $résultat = saluer($café);
#  ^ @variable
