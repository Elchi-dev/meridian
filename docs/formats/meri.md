# The .meri format

Version 1

`.meri` is how Meridian writes configuration and system files: sextant's settings, the bootloader's settings, and later every settings file of the system itself. It is meant to be read and edited by people and parsed by programs in one pass, without backtracking, in a few hundred lines of code and without allocating for anything but the result.

```meri
# Settings for sextant, the Meridian build tool.

qemu {
    memory = 512MiB
    cpus = 4
    accel = kvm               # a bare word: one of a fixed set of choices
    firmware = "/usr/share/edk2/x64/OVMF_CODE.4m.fd"
}

display "DP-1" {
    scale = 125%
    refresh = 60Hz
}

boot { timeout = 3s }

motd = """
    Welcome to Meridian.
      Indentation inside the text is kept.
    """

features = [
    "snapshots",
    "dedup",   # comments work inside lists too
]
```

This document is the reference. A program that accepts exactly the documents described here, rejects everything else at the position given here, and produces the dump described in section 15 is a conforming implementation. `tests/meri` holds the conformance suite.

## 1. Encoding

A document is a sequence of bytes that must be valid UTF-8. Invalid UTF-8 anywhere, including inside comments and strings, is an error (`invalid-utf8`), reported at the first byte of the invalid sequence. Surrogate code points encoded in UTF-8 and overlong encodings are invalid UTF-8.

If the document starts with the byte order mark (the bytes `EF BB BF`), it is skipped and does not count as a character for column numbers. A byte order mark anywhere else is an ordinary character, which means it is allowed inside strings and comments and is an error (`unexpected-character`) anywhere else.

## 2. Characters, lines and positions

Lines end with a line feed (`LF`, U+000A) or with a carriage return followed by a line feed (`CR LF`). A carriage return that is not followed by a line feed is an error (`lone-carriage-return`).

Apart from line endings, the only control character allowed in a document is the tab (U+0009). Every other character from U+0000 to U+001F, and U+007F, is an error (`control-character`) wherever it appears, in strings and comments too. Strings can still contain these characters through escapes.

Positions are a line and a column, both starting at 1. Each line ending starts a new line. The column counts Unicode scalar values from the start of the line, so a tab counts as one column and a multi-byte character counts as one.

Whitespace is the space (U+0020) and the tab. A comment starts with `#` outside a string and runs to the end of the line. Comments may appear wherever whitespace may, and at the end of any line.

## 3. Structure

A document is a block without a name, called the root. A block holds items, and every item is either an entry or a nested block:

```
entry  =  key  "="  value
block  =  name  [ label ]  "{"  items  "}"
```

Whitespace may appear between any two of these parts, but never a line ending: a key, its `=` and the start of its value are on one line, and so are a block's name, its label and its `{`.

Every item ends at the end of its line, or right before a `}` on the same line. After a complete item, only whitespace and a comment may follow before the line ends or before that `}`. Anything else is an error (`expected-newline`). Two items therefore never share a line, but a short block fits on one:

```meri
boot { timeout = 3s }
outer { inner { level = 2 } }
```

A block's items start after its `{`, on the same line or on the next. A block ends at its matching `}`. Blank lines, whitespace and comments are allowed anywhere between items. Indentation has no meaning.

A `}` with no open block is an error (`unexpected-close`). Reaching the end of the document inside a block is an error (`unclosed-block`), reported at the name of the innermost open block.

## 4. Keys and names

Keys and block names follow the same rule: a letter (`A` to `Z`, `a` to `z`) or `_`, followed by any number of letters, digits, `_` and `-`. They are case-sensitive, so `Size` and `size` are different keys.

A line that should start an item but starts with any other character is an error (`unexpected-character`). After a key, the next thing on the line must be `=`, `{` or a label. Anything else, including the end of the line, is an error (`expected-equals`). After a label, the next thing on the line must be `{`; anything else is an error (`expected-brace`).

Within one block:

- A key may be set only once. Setting it again is an error (`duplicate-key`), reported at the second key.
- A name is either a key or the name of blocks, never both. Using it for the other kind is an error (`name-conflict`), reported where the second kind appears.
- Blocks may repeat. Unlabelled blocks with the same name form an ordered sequence. Labelled blocks with the same name must have different labels; repeating a name and label is an error (`duplicate-label`), reported at the second block's name.

## 5. Labels

A label sits between a block's name and its `{` and is either a string (section 9, without the text form) or a bare word (section 7). A label written as a word and a label written as a string with the same characters are the same label. `true` and `false` in label position are simply words.

## 6. Values

A value starts right after `=` (after optional whitespace). It is one of:

| Type | Example |
| --- | --- |
| boolean | `true`, `false` |
| word | `kvm`, `dark`, `x86_64` |
| integer | `4`, `-12`, `1_000_000`, `0xff`, `0o755`, `0b1010` |
| float | `1.5`, `-0.25`, `6.02e23` |
| quantity | `512MiB`, `30s`, `1.5h`, `60Hz`, `125%` |
| string | `"text"`, and the multi-line text form |
| list | `[1, 2, 3]` |

If the line ends, or a comment, `}` or the end of the document comes where a value should start, that is an error (`expected-value`), reported where the value should have started. Inside a list the same applies to `,` and `]`. Any other character that cannot start a value is an error (`unexpected-character`).

## 7. Booleans and words

A word follows the same rule as a key. The two words `true` and `false` are booleans; every other word is a value of type word. Words are meant for a fixed set of choices (`accel = kvm`); anything free-form belongs in a string.

## 8. Numbers

### 8.1 Integers

A decimal integer is an optional `-` followed by digits. `_` may separate digits: it must have a digit on both sides, so `1_000` is valid and `_1`, `1_`, `1__0` are not. A decimal integer with more than one digit cannot start with `0` (`007` is an error; write `7`, or `0o7` for octal).

An integer may also be written as `0x` with hexadecimal digits (`0` to `9`, `a` to `f`, `A` to `F`), `0o` with octal digits or `0b` with binary digits, again with an optional `-` in front and `_` between digits. The prefixes are lower case, and a prefix only counts as one when the character right after it is a digit of its base. Otherwise the `0` is a decimal number and the letter starts a unit: `0bit` is the quantity 0 with the unit `bit`, `0B` is 0 with the unit `B`, and `0o8` is invalid because `8` follows the unit `o`.

Every integer must fit a signed 64-bit integer, from -9223372036854775808 to 9223372036854775807. A value outside that range is an error (`number-overflow`). `-0x8000_0000_0000_0000` is the smallest integer.

Any malformed number is an error (`invalid-number`) reported where the number starts. That includes a `-` without a digit after it, a digit that does not belong to the base, a misplaced `_`, and a letter, digit, `_`, `.` or `%` that directly follows a complete hexadecimal, octal or binary number. As a single exception, a letter or `%` directly after a hexadecimal, octal or binary number that is not a digit of its base is reported as `unit-not-allowed`, because units only exist for decimal numbers.

### 8.2 Floats

A float is a decimal integer part (with the same rules for `-`, `_` and leading zeros) followed by a fraction, an exponent, or both:

- The fraction is `.` followed by digits, with `_` between digits allowed. Digits are required on both sides of the point: `.5` and `5.` are errors.
- The exponent is `e` or `E`, an optional `+` or `-`, and digits, with `_` between digits allowed. An `e` or `E` only starts an exponent when it is followed by a digit, or by a sign and a digit. Otherwise it starts a unit (section 8.3), so `5em` is the quantity 5 with the unit `em`.

The value is the IEEE 754 double closest to the decimal number. A value too large to be represented is an error (`number-overflow`); a value too small rounds to zero or to a subnormal number without error.

### 8.3 Quantities

A quantity is a decimal integer or a float immediately followed by a unit, with no space between them. A unit is either one or more ASCII letters, or the single character `%`. Its number follows all the rules of 8.1 and 8.2, including the range of a 64-bit integer when it is written as an integer.

The format gives no meaning to units. A quantity is a number and a unit, and any unit is accepted, so a document can say `refresh = 60Hz` or `ratio = 3x`. Programs that read a quantity decide which units they accept (section 13 lists the ones Meridian uses for sizes and durations).

After a number, with or without a unit, the next character must not be a letter, a digit, `_`, `.` or `%`. If it is, the number is an error (`invalid-number`): `1.2.3`, `5MiB2` and `5%%` are all invalid.

## 9. Strings

### 9.1 Quoted strings

A string is enclosed in double quotes and stays on one line. Inside it, every character except `"`, `\`, line endings and the forbidden control characters stands for itself. A tab is allowed.

| Escape | Meaning |
| --- | --- |
| `\\` | backslash |
| `\"` | double quote |
| `\n` | line feed |
| `\r` | carriage return |
| `\t` | tab |
| `\0` | U+0000 |
| `\u{XXXX}` | the Unicode scalar value with 1 to 6 hexadecimal digits XXXX |

`\u{}` must contain 1 to 6 hexadecimal digits and name a Unicode scalar value: U+D800 to U+DFFF and anything above U+10FFFF are not allowed (`invalid-unicode-escape`). Anything else after `\`, including the end of the line or of the document, is an error (`invalid-escape`). Both are reported at the backslash.

A string that reaches the end of its line or of the document before its closing quote is an error (`unterminated-string`), reported at the opening quote.

### 9.2 Text

Longer text, such as a message or a script, uses three double quotes:

```meri
motd = """
    Welcome to Meridian.
      Indentation inside the text is kept.
    """
```

- The opening `"""` must be the last thing on its line, apart from whitespace. Anything else after it, including a comment, is an error (`text-opening`), reported at that character.
- The lines that follow are the content, up to the first line that consists of whitespace followed by `"""`. That closing line ends the text. After the closing `"""` the usual rules for the end of an item apply, so it may be followed by whitespace, a comment, a `}`, or inside a list by `,` or `]`.
- The whitespace before the closing `"""` is the indentation. It is removed from the start of every content line. A content line made only of spaces and tabs becomes an empty line, whatever its length. Any other content line must start with exactly the same whitespace characters as the indentation; if it does not, that is an error (`text-indent`) reported at column 1 of that line.
- The content lines are joined with line feeds. The line ending before the closing line is not part of the text, so text never ends in a line feed unless an empty content line is placed before the closing line. Line endings in the content are always line feeds, whether the document uses `LF` or `CR LF`.
- No escapes are processed in text; a backslash is a backslash. Tabs are allowed, other control characters are not.
- A document that ends before the closing line is an error (`unterminated-text`), reported at the opening `"""`.
- The indentation of the content lines can only be checked once the closing line is known. Every other error inside a text is therefore reported before a `text-indent` error, even when it comes later in the text.

Text and quoted strings produce the same type of value.

## 10. Lists

A list is `[`, values separated by `,`, and `]`. Line endings, whitespace and comments may appear between any of its parts, so a list may span many lines. A comma after the last value is allowed, and `[]` is the empty list. A list may hold values of different types, including other lists, but never blocks.

A list that reaches the end of the document before its `]` is an error (`unterminated-list`), reported at its `[`. Two values without a comma between them are an error (`expected-comma`), reported where the second one starts. A `,` or `]` where a value should start is an error (`expected-value`).

## 11. Limits

Blocks and lists may nest at most 64 levels deep. The root is level 0, and every block or list opens a new level, so the deepest block inside the root is at level 64. Opening a 65th level is an error (`too-deep`), reported at the name of the block or at the `[` of the list. The limit lets implementations without a heap use a fixed stack.

## 12. Errors

An error stops parsing; only the first one in a document is reported. Each error has a position and a code. The codes below are part of this specification and appear in the conformance suite. The messages are for people and may be worded by each implementation, but should say what to do, for example:

```
sextant.meri:4:5: "cpus" is already set on line 2; remove one of them
```

| Code | Meaning |
| --- | --- |
| `invalid-utf8` | bytes that are not valid UTF-8 |
| `control-character` | a control character other than tab and line endings |
| `lone-carriage-return` | a carriage return without a line feed after it |
| `unexpected-character` | a character that cannot start an item or a value |
| `expected-equals` | a key without `=`, `{` or a label after it |
| `expected-brace` | a label without `{` after it |
| `expected-value` | no value where one must start |
| `expected-newline` | something after a complete item on the same line |
| `expected-comma` | two list values without a comma between them |
| `duplicate-key` | a key set twice in one block |
| `name-conflict` | a name used for a key and for blocks in one block |
| `duplicate-label` | two blocks with the same name and label in one block |
| `unclosed-block` | the document ends inside a block |
| `unexpected-close` | a `}` without an open block |
| `unterminated-string` | a string without its closing quote on the same line |
| `invalid-escape` | an unknown escape in a string |
| `invalid-unicode-escape` | a malformed `\u{...}` or one naming no Unicode scalar value |
| `text-opening` | something after the opening `"""` on its line |
| `text-indent` | a text line indented less than its closing `"""` |
| `unterminated-text` | the document ends inside a text |
| `unterminated-list` | the document ends inside a list |
| `invalid-number` | a malformed number |
| `number-overflow` | a number outside the range of its type |
| `unit-not-allowed` | a unit after a hexadecimal, octal or binary number |
| `too-deep` | more than 64 levels of blocks and lists |

## 13. Reading values

How a program interprets values is up to the program, but Meridian's readers all follow these rules:

- A setting that is not in the document takes its default. A setting that is present with the wrong type is an error naming the file, the position of the value, the key, what was expected and what was found: `sextant.meri:4:14: memory must be a size such as 512MiB, found the string "lots"`.
- Where text is expected, both strings and words are accepted.
- Where a float is expected, integers are accepted.
- Sizes are quantities in bytes with one of these units: `B`, `KB` (1000), `MB`, `GB`, `TB`, `KiB` (1024), `MiB`, `GiB`, `TiB`. The result must be a whole, non-negative number of bytes that fits a signed 64-bit integer, so `1.5KiB` is 1536 bytes and `1.5B` is an error. Conversion is exact; there is no floating point rounding.
- Durations are quantities with one of these units: `ns`, `us`, `ms`, `s`, `min`, `h`, `d` (24 hours). The result must be a whole number of nanoseconds that fits a signed 64-bit integer. Negative durations are allowed.
- Units are case-sensitive: `mib` is not a size unit.
- A value of the wrong type, or a size or duration with an unknown unit, is reported with the code `wrong-type`. A size or duration that is not a whole number of bytes or nanoseconds, a negative size, or one too large for 64 bits is reported with `out-of-range`.

## 14. Layering

Settings that apply to one machine, such as `sextant.local.meri`, are layered over the shared ones. Merging an override document into a base document gives a new document:

- An entry in the override replaces the item with the same name in the base, whatever kind that item was, and keeps the base's position in the order. Entries the base does not have are added at the end.
- A labelled block in the override is merged into the base block with the same name and label, recursively. If the base has none, it is added at the end.
- Unlabelled blocks are merged one to one when the base and the override each have exactly one with that name. If either side has more than one, the override's unlabelled blocks with that name replace all of the base's.
- If the base has an entry where the override has blocks of the same name, the override's blocks replace that entry.

Merging keeps every value's original position, so an error about a merged value points to the file it came from.

## 15. Canonical dump

The conformance suite compares documents through a canonical dump: one line per item, in document order, depth first, each line ending in a line feed. An empty document dumps to nothing.

Every item has a path. The path of an item in the root is its name. The path of an item inside a block is the block's path, a `.`, and the item's name. A block's own path segment is its name followed by `#` and its index among the blocks with that name in the same parent, counting from 0. List elements are written after their list, with the list's path followed by `[index]`.

```
<path> block
<path> block "<label>"
<path> bool true
<path> int <value>
<path> float <number>
<path> quantity <number> <unit>
<path> string "<text>"
<path> word <word>
<path> list <length>
```

- `int` values are written in decimal without separators or leading zeros, with `-` for negative values. `-0` is `0`.
- The `<number>` of a float, and of a quantity written as a float, is the number exactly as written in the document with every `_` removed, `E` replaced by `e` and a `+` after the `e` removed. The number of a quantity written as an integer follows the rule for `int`.
- In labels and strings, `\` is written `\\`, `"` is written `\"`, line feed `\n`, carriage return `\r`, tab `\t`, U+0000 `\0`, and every other character from U+0001 to U+001F and U+007F as `\u{X}` with lower-case hexadecimal digits and no leading zeros. All other characters are written as themselves.

```meri
qemu {
    memory = 512MiB
    display "DP-1" { scale = 125% }
}
tags = [fast, "a b"]
```

dumps to

```
qemu#0 block
qemu#0.memory quantity 512 MiB
qemu#0.display#0 block "DP-1"
qemu#0.display#0.scale quantity 125 %
tags list 2
tags[0] word fast
tags[1] string "a b"
```

## 16. Conformance suite

`tests/meri/valid` holds documents that must parse, each `name.meri` with the expected dump in `name.dump`. `tests/meri/invalid` holds documents that must be rejected, each `name.meri` with `name.error` containing one line: `<line>:<column> <code>`. These files are stored byte for byte; git must never convert their line endings.
