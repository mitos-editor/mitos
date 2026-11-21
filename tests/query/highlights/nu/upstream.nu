type count = int
# ^ @keyword.storage.type
#    ^ @type

ls | any size > 0
#    ^ @function.builtin
ls | all size > 0
#    ^ @function.builtin

peek
# ^ @function.builtin
collect { $in }
# ^ @keyword
