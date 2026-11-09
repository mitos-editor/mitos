int draw(void);
//  ^ @function.declaration
int draw(void) {
//  ^ @function.definition
    draw();
//  ^ @function.call
    return 0;
}
char *name(void) { return 0; }
//    ^ @function.definition
