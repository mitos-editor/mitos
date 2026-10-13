Write-Host hello && Write-Host world
#                ^^ @operator
Write-Host hello || Write-Host world
#                ^^ @operator
$object.Method()
#       ^^^^^^ @function.method
$object.Property
#       ^^^^^^^^ @variable.other.member
class Example {}
#     ^^^^^^^ @type
