(tag
 (name) @comment
 (user)? @constant)

; Hint level tags
((tag (name) @comment.hint)
 (#any-of? @comment.hint "HINT" "MARK" "PASSED" "STUB" "MOCK" "TIP" "IDEA" "EVIDENCE-OF" "COM" "CHECK" "CHECK-NEXT" "CHECK-SAME" "CHECK-EMPTY" "CHECK-NOT" "CHECK-COUNT" "CHECK-DAG" "CHECK-LABEL"))

("text" @comment.hint
 (#any-of? @comment.hint "HINT" "MARK" "PASSED" "STUB" "MOCK" "TIP" "IDEA" "EVIDENCE-OF" "COM" "CHECK" "CHECK-NEXT" "CHECK-SAME" "CHECK-EMPTY" "CHECK-NOT" "CHECK-COUNT" "CHECK-DAG" "CHECK-LABEL"))

; Info level tags
((tag (name) @comment.info)
 (#any-of? @comment.info "INFO" "NOTE" "TODO" "TO-DO" "PERF" "OPTIMIZE" "PERFORMANCE" "QUESTION" "ASK" "REVIEW" "PR" "CR" "MAGIC" "WHY" "REASON" "NB" "N.B."))

("text" @comment.info
 (#any-of? @comment.info "INFO" "NOTE" "TODO" "TO-DO" "PERF" "OPTIMIZE" "PERFORMANCE" "QUESTION" "ASK" "REVIEW" "PR" "CR" "MAGIC" "WHY" "REASON" "NB" "N.B."))

; Warning level tags
((tag (name) @comment.warning)
 (#any-of? @comment.warning "HACK" "WARN" "WARNING" "TEST" "TESTING" "TEMP" "CAREFUL" "CAUTION" "TRICKY" "FRAGILE"))

("text" @comment.warning
 (#any-of? @comment.warning "HACK" "WARN" "WARNING" "TEST" "TESTING" "TEMP" "CAREFUL" "CAUTION" "TRICKY" "FRAGILE"))

; Error level tags
((tag (name) @comment.error)
 (#any-of? @comment.error "BUG" "FIXME" "ISSUE" "XXX" "FIX" "SAFETY" "FIXIT" "FAILED" "DEBUG" "INVARIANT" "COMPLIANCE" "PANIC" "SECURITY" "ASSUMPTION" "ASSUME" "POSTCONDITION" "PRECONDITION" "PROOF"))

("text" @comment.error
 (#any-of? @comment.error "BUG" "FIXME" "ISSUE" "XXX" "FIX" "SAFETY" "FIXIT" "FAILED" "DEBUG" "INVARIANT" "COMPLIANCE" "PANIC" "SECURITY" "ASSUMPTION" "ASSUME" "POSTCONDITION" "PRECONDITION" "PROOF"))

; Issue number (#123)
("text" @constant.numeric
 (#match? @constant.numeric "^#[0-9]+$"))

; User mention (@user)
("text" @tag
 (#match? @tag "^[@][a-zA-Z0-9_-]+$"))

(uri) @markup.link.url
