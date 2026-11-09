class Widget:
#     ^ @type.class.definition
    def paint(self):
#       ^ @function.method.definition
        self.paint()
#            ^ @function.method.call
    @decorator
#    ^ @attribute
    def decorated(self):
#       ^ @function.method.definition
        pass
def draw():
#   ^ @function.definition
    draw()
#   ^ @function.call

def documented():
    """Summary."""
#    ^ @string
    x = "ordinary"
#        ^ @string
