[
  "Output"
  "Backspace"
  "Down"
  "Enter"
  "Escape"
  "Left"
  "Right"
  "Space"
  "Tab"
  "Up"
  "Set"
  "Type"
  "Sleep"
  "Env"
  "Wait"
  "Require"
  "Source"
  "Copy"
  "Paste"
  "PageUp"
  "PageDown"
  "Hide"
  "Show" ] @keyword

[ "Shell"
  "FontFamily"
  "FontSize"
  "Framerate"
  "PlaybackSpeed"
  "Height"
  "LetterSpacing"
  "TypingSpeed"
  "LineHeight"
  "Padding"
  "Theme"
  "LoopOffset"
  "Width"
  "BorderRadius"
  "Margin"
  "MarginFill"
  "WindowBar"
  "WindowBarSize"
  "CursorBlink" ] @type

[ "@" ] @operator
[(control) (alt) (shift)] @function.macro
(float) @constant.numeric.float
(integer) @constant.numeric.integer
(comment) @comment
[(string) (json)] @string
(regex) @string.regexp
(path) @string.special.path
(time) @string.special.symbol
(boolean) @constant.builtin.boolean
(waitOn) @keyword
