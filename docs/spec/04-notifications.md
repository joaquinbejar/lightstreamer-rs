# TLCP Notification Reference — Updates, Subscriptions, Messages, Session

Scope: this chapter distills chapter 3 (*Notification Reference*) of the **TLCP Specification, version 2.5.0**, covering printed pages 49–65, **excluding** the *MPN-Related Notifications* section (printed pages 57–59, tags `MPNREG`, `MPNZERO`, `MPNOK`, `MPNCONF`, `MPNDEL`), which is covered in a separate chapter. It additionally reproduces **Appendix D** (printed pages 98–99), because the `^T` diff format used by real-time updates is defined there and nowhere else. Every factual statement below carries a citation of the form `[§Section Name, p.NN]`, where `NN` is the **printed** page number of the specification. Wire tokens are reproduced verbatim in backticks with exact casing and separators; every spec example is quoted verbatim in a fenced block so it can be lifted directly into a parser test fixture.

---

## 1. General Notification Syntax

Every notification uses the same base syntax `[§3 Notification Reference, p.49]`:

```
<tag>,<argument1>,...,<argumentN>
```

- Line separator is **CR-LF** `[§3 Notification Reference, p.49]`.
- Encoding is **UTF-8** `[§3 Notification Reference, p.49]`.
- With HTTP transport the notification is in the body of the HTTP response `[§3 Notification Reference, p.49]`.
- `<tag>` is a short (up to 8 characters) uppercase string identifying the kind of response or notification `[§Common Response and Notification Syntax, p.13]`.
- Each different tag has a **fixed number of arguments**; once the tag has been extracted by finding the first comma, the number of remaining commas is known `[§Common Response and Notification Syntax, p.13]`.
- Arguments containing meta characters such as the comma are **percent-encoded**, with the notable exception of the last argument of a real-time update `[§Common Response and Notification Syntax, p.13]`.
- An argument may be empty, although most arguments will never be empty because of their meaning `[§Common Response and Notification Syntax, p.13]`.

The spec's first-pass parsing algorithm, relevant to every notification in this chapter `[§Common Response and Notification Syntax, p.14]`:

> - Read the HTTP response or the WS message line by line (line terminator is CR-LF).
> - For each line, trim the trailing line terminator and look for the first comma from left to right (it may not be there).
> - The text until the first comma (or the full line, if it's not there) is the tag.
> - Switch on the tag:
>   - Each tag has a fixed number of arguments (e.g. for a "REQERR" tag it's 3).
>   - Split the remaining part of the string comma by comma, from left to right, to obtain each argument. Note that the last argument may contain additional commas.
>   - Percent-decode non-numeric arguments, except for the last argument of a real-time update (tag "U").

The spec's own footnote on that exception `[§Common Response and Notification Syntax, p.14]`:

> The last argument of a real-time update is itself a variable-length set of fields, with its own second-level syntax. To avoid unnecessary encoding of frequently used characters, this argument follows specific encoding rules and should not be percent-decoded during the first pass.

The spec also notes that the first 4 characters of a tag are unique, permitting a 32-bit integer switch `[§Common Response and Notification Syntax, p.14]`.

---

## 2. Real-Time Update — `U`

### 2.1 Tag, formats and arguments

**Tag:** `U` (short of Update) `[§Real-Time Update, p.49]`

**Formats** (all four are the same notification; they differ only in the second-level syntax of the third argument) `[§Real-Time Update, p.49]`:

```
U,<subscription-ID>,<item>,<field-1-value>|<field-2-value>|...|<field-N-value>
U,<subscription-ID>,<item>,<field-1-value>|^<number-of-unchanged-fields>|...|<field-N-value>
U,<subscription-ID>,<item>,<field-1-value>|^P<JSON Patch value>|...|<field-N-value>
U,<subscription-ID>,<item>,<field-1-value>|^T<TLCP-diff value>|...|<field-N-value>
```

**Used when:** to send a real-time update on the content of the fields of an item. The notification is also used to send the item's snapshot `[§Real-Time Update, p.49]`.

**Arguments: 3** `[§Real-Time Update, p.49]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.49]`. |
| 2 | `<item>` | numeric, **1-based** | Index of the item being updated. The order of items in the subscription is determined by the Metadata Adapter `[p.49]`. |
| 3 | `<field-1-value>\|...\|<field-N-value>` | pipe-separated list | Pipe-separated list of field values, with its own second-level syntax (§2.2). The order of fields in the subscription is determined by the Metadata Adapter `[p.49]`. |

Because arguments 1 and 2 are numeric they are not percent-decoded in the first pass, and argument 3 is explicitly exempted from first-pass percent-decoding `[§Common Response and Notification Syntax, p.14]`.

### 2.2 The pipe-separated value list — encoding rules

The third argument contains the field values for the specified item. Its second-level syntax is designed to be as compact as possible `[§Decoding the Pipe-Separated List of Values, p.49]`.

The encoding rules, verbatim in substance `[§Decoding the Pipe-Separated List of Values, pp.49–50]`:

| Token in the list | Meaning |
|---|---|
| *(empty)* | The field is **unchanged** compared to the previous update of the same field `[p.49]`. |
| `#` | The field is **null** `[p.49]`. |
| `$` | The field is **empty** `[p.49]`. |
| `^` followed by a **number**, e.g. `^3` | The following number of fields are **unchanged** `[p.50]`. |
| `^` followed by a **letter**, e.g. `^P` | The remaining part of the value represents the **difference** between the previous and new update, expressed in a "diff" format, where the letter is the tag of the format used `[p.50]`. |
| anything else | Actual content `[p.50]`. |

Additional rules:

- Each value is a **UTF-8 string** `[§Decoding the Pipe-Separated List of Values, p.49]`.
- When a `^`+letter diff is received, the real update value should be computed based on the previous update of the same field and the provided difference, according to the format `[§Decoding the Pipe-Separated List of Values, p.50]`.
- **Meta characters, such as the pipe `|`, CR-LF, etc., are percent-encoded** `[§Decoding the Pipe-Separated List of Values, p.50]`.
- `#`, `$` and `^` are percent-encoded **if occurring at the beginning of an actual content (and only in this case)** `[§Decoding the Pipe-Separated List of Values, p.51]`.

The two available diff format tags `[§Decoding the Pipe-Separated List of Values, p.50]`:

| Letter | Format | Definition |
|---|---|---|
| `P` | **JSON Patch** | The value format is specified by RFC 6902 `[p.50]`. |
| `T` | **TLCP-diff** | The value format is a custom encoding, specified in Appendix D `[p.50]`. |

Guarantees attached to each `[§Decoding the Pipe-Separated List of Values, p.50]`:

> P – JSON Patch
> The value format is specified by RFC 6902.
> The JSON Patch algorithm can be applied only between values that are valid JSON representations. Hence, upon reception of a JSON Patch, it is guaranteed that the previous update value is a valid JSON representation (and, obviously, the new update value will be a valid JSON representation too).

> T – TLCP-diff
> The value format is a custom encoding. The format is specified in Appendix D.
> This format can be applied to all strings, only provided that their encoding in UTF-16 (the format used internally in Java) doesn't contain surrogate pairs. Obviously, if a TLCP-diff is received, this guarantees that both the previous and new value are compliant.

### 2.3 Decoding algorithm

Quoted verbatim from the spec, which labels it "a simple decoding algorithm" `[§Decoding the Pipe-Separated List of Values, pp.50–51]`:

> - Set a pointer to the first field of the schema.
> - Look for the next pipe "|" from left to right and take the substring to it, or to the end of the line if no pipe is there.
> - Evaluate the substring:
>   - If its value is empty, the pointed field should be left **unchanged** and the pointer moved to the next field.
>     - Note: this case never happens on the first update of a subscription.
>   - Otherwise, if its value corresponds to a single hash sign "#" (UTF-8 code 0x23), the pointed field should be set to a **null value** and the pointer moved to the next field.
>   - Otherwise, If its value corresponds to a single dollar sign "$" (UTF-8 code 0x24), the pointed field should be set to an **empty value** ("") and the pointer moved to the next field.
>   - Otherwise, if its value begins with a caret "^" (UTF-8 code 0x5E):
>     - Note: this case never happens on the first update of a subscription.
>     - check if the following character is a letter or a digit
>     - case digit:
>       - take the substring following the caret and convert it to an integer number;
>       - for the corresponding count, leave the fields **unchanged** and move the pointer forward;
>       - e.g. if the value is "^3", leave unchanged the pointed field and the following two fields, and move the pointer 3 fields forward.
>     - case letter:
>       - Note: this case never happens if the value of the pointed field is currently null.
>       - take the substring following the letter and decode any **percent-encoding**
>       - use the obtained value as a difference to be applied to the value of the pointed field, based on the format specified by the letter
>       - set the pointed field to the value finally obtained
>   - Otherwise, the value is an actual content: decode any **percent-encoding** and set the pointed field to the decoded value, then move the pointer to the next field.
>     - Note: "#", "$" and "^" characters are percent-encoded if occurring at the beginning of an actual content (and only in this case).
> - Return to the second step, unless there are no more fields in the schema.

Decoding invariants that follow directly from the quoted text and matter for byte-identical output:

1. **Splitting comes first.** The list is split on **unescaped** `|` characters; a `|` that is part of a value is percent-encoded as `%7C` and therefore does not act as a separator `[§Decoding the Pipe-Separated List of Values, p.50]`. This is visible in the diff example on p.52, where `aa%7Cbb` decodes to `aa|bb`.
2. **The marker test is exact, not prefix-based**, for `#` and `$`: the substring must correspond to a *single* hash sign / a *single* dollar sign `[p.50]`. A value `#abc` is therefore actual content, and a literal value that *is* `#` at the start of actual content arrives percent-encoded `[p.51]`.
3. **The `^` test is prefix-based**: "begins with a caret" `[p.50]`.
4. **Percent-decoding is applied last**, to actual content and to the payload of a `^`+letter diff `[pp.50–51]`. It is *not* applied to `#`, `$`, or `^`+digit markers, which carry no payload.
5. **One list token does not always equal one field.** `^N` advances the field pointer by `N` while consuming a single token; every other token advances by exactly one `[p.50]`.
6. `^3` means "the pointed field **and** the following two", i.e. the count is inclusive of the field currently pointed at `[p.50]`.
7. The empty-token and `^`-token cases never occur on the first update of a subscription `[p.50]`; the `^`+letter case never occurs when the pointed field is currently null `[p.50]`.

⚠️ **Spec unclear:** the algorithm terminates when "there are no more fields in the schema" `[p.51]`, and all worked examples supply tokens covering the full schema. The spec does not state what a client must do if the line ends **before** the schema is exhausted (implicitly-unchanged trailing fields vs. protocol error), nor if the tokens would advance the pointer **past** the end of the schema (e.g. a `^N` whose count overruns the remaining fields) `[§Decoding the Pipe-Separated List of Values, pp.50–51]`.

⚠️ **Spec unclear:** the algorithm says to "check if the following character is a letter or a digit" after a `^` `[p.50]` but does not define the behaviour when the character following `^` is neither (nor when the letter is a tag other than `P` or `T`) `[§Decoding the Pipe-Separated List of Values, p.50]`.

⚠️ **Spec unclear:** for the `^`+digit case the spec says to "take the substring following the caret and convert it to an integer number" `[p.50]` without constraining that substring's format (sign, leading zeros, maximum magnitude, radix) beyond the `^3` example `[§Decoding the Pipe-Separated List of Values, p.50]`.

⚠️ **Spec unclear:** the meta characters that are percent-encoded inside the value list are given only by example — "such as the pipe `|`, CR-LF, etc." `[p.50]` — and the exhaustive set is not enumerated. Note in particular that the comma is **not** required to be encoded here, since the third argument of `U` is the last argument and may contain additional commas `[§Common Response and Notification Syntax, p.14]`; the percent sign itself is not discussed for this second-level syntax `[§Decoding the Pipe-Separated List of Values, p.50]`.

⚠️ **Spec unclear:** the `^`+letter case is stated to never happen when the pointed field is currently **null** `[p.50]`, but nothing is stated about applying a diff when the pointed field is currently the **empty** value, nor about applying a diff to a field that has not yet received any update `[§Decoding the Pipe-Separated List of Values, p.50]`.

### 2.4 Snapshot vs real-time updates

`U` carries both snapshot information and real-time updates; the snapshot may be made of 0 or multiple such notifications depending on the subscription mode `[§Snapshot vs Real-Time Updates, p.51]`. The spec's decision table, reproduced verbatim `[§Snapshot vs Real-Time Updates, p.51]`:

| LS_snapshot | LS_mode | EOS notification already received? | First notification? | The notification is... |
|---|---|---|---|---|
| `false` or missing | - | (cannot be received) | - | real-time update |
| `true` or a number (a number is only meaningful for `DISTINCT` mode) | `RAW` | (cannot be received) | - | real-time update |
| " | `MERGE` | (cannot be received) | YES | snapshot |
| " | `MERGE` | (cannot be received) | NO | real-time update |
| " | `DISTINCT` | YES | - | real-time update |
| " | `DISTINCT` | NO | - | snapshot |
| " | `COMMAND` | YES | - | real-time update |
| " | `COMMAND` | NO | - | snapshot |

(In the printed table the first column cell `true or a number` spans all seven `RAW`/`MERGE`/`DISTINCT`/`COMMAND` rows, and the `MERGE` cell spans its two `First notification?` rows; the `"` marks above denote that span `[§Snapshot vs Real-Time Updates, p.51]`.)

### 2.5 Notification examples (no diffs)

Setup, verbatim `[§Notification Examples, p.51]`:

> As an example, suppose a subscription to a typical stock quote adapter, with subscription ID 3, just one item, and the following fields in the schema: `timestamp`, `price`, `change`, `minimum`, `maximum`, `bid`, `ask`, `open`, `close` and `status`.

**Example 1** `[§Notification Examples, p.51]`:

```
U,3,1,20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:00:33` | `3.04` | `0.0` | `2.41` | `3.67` | `3.03` | `3.04` | *<null>* | *<null>* | *<empty>* |
| Changed | Changed | Changed | Changed | Changed | Changed | Changed | Changed | Changed | Changed |

> The initial update (the snapshot) carries information for all the fields. It contains two null fields (`open` and `close`) and an empty field (`status`).

**Example 2** `[§Notification Examples, pp.51–52]`:

```
U,3,1,20:00:54|3.07|0.98|||3.06|3.07|||Suspended
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:00:54` | `3.07` | `0.98` | `2.41` | `3.67` | `3.06` | `3.07` | *<null>* | *<null>* | `Suspended` |
| Changed | Changed | Changed | Unchanged | Unchanged | Changed | Changed | Unchanged | Unchanged | Changed |

> This update contains 4 unchanged fields: `minimum`, `maximum`, `open` and `close`.

**Example 3** `[§Notification Examples, p.52]`:

```
U,3,1,20:04:16|3.02|-0.65|||3.01|3.02|||$
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:04:16` | `3.02` | `-0.65` | `2.41` | `3.67` | `3.01` | `3.02` | *<null>* | *<null>* | *<empty>* |
| Changed | Changed | Changed | Unchanged | Unchanged | Changed | Changed | Unchanged | Unchanged | Changed |

> This update contains again 4 unchanged fields: `minimum`, `maximum`, `open` and `close`. Moreover, it sets he `status` field to its initial empty value.

(The "sets he" typo is verbatim from the printed page `[p.52]`.)

**Example 4 — contiguous unchanged run** `[§Notification Examples, p.52]`:

```
U,3,1,20:04:40|^4|3.02|3.03|||
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:04:40` | `3.02` | `-0.65` | `2.41` | `3.67` | `3.02` | `3.03` | *<null>* | *<null>* | *<empty>* |
| Changed | Unchanged | Unchanged | Unchanged | Unchanged | Changed | Changed | Unchanged | Unchanged | Unchanged |

> This update includes the special syntax for multiple contiguous unchanged fields: `price`, `change`, `minimum` and `maximum`. The `open`, `close` and `status` fields are also unchanged.

**Example 5 — run extending to end of schema** `[§Notification Examples, p.52]`:

```
U,3,1,20:06:10|3.05|0.32|^7
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:06:10` | `3.03` | `-0.32` | `2.41` | `3.67` | `3.02` | `3.03` | *<null>* | *<null>* | *<empty>* |
| Changed | Changed | Changed | Unchanged | Unchanged | Unchanged | Unchanged | Unchanged | Unchanged | Unchanged |

> This update includes again the special syntax for multiple contiguous unchanged fields: `minimum`, `maximum`, `bid`, `ask`, `open`, `close` and `status`.

⚠️ **Spec unclear:** this example is internally inconsistent. The wire line carries `3.05` for `price` and `0.32` for `change`, but the accompanying table shows `3.03` and `-0.32` for those two fields (both marked *Changed*) `[§Notification Examples, p.52]`. The structural point of the example — that `^7` covers the last seven fields — is unaffected, but the literal values must not be used as a fixture without resolving which side is wrong.

**Example 6** `[§Notification Examples, p.52]`:

```
U,3,1,20:06:49|3.08|1.31|||3.08|3.09|||
```

| timestamp | price | change | minimum | maximum | bid | ask | open | close | status |
|---|---|---|---|---|---|---|---|---|---|
| `20:06:49` | `3.08` | `1.31` | `2.41` | `3.67` | `3.08` | `3.09` | *<null>* | *<null>* | *<empty>* |
| Changed | Changed | Changed | Unchanged | Unchanged | Changed | Changed | Unchanged | Unchanged | Unchanged |

> The final update contains once more 4 unchanged fields: `minimum`, `maximum`, `open` and `close`.

### 2.6 Notification examples with the use of "diff" values

Setup, verbatim `[§Notification Examples with the use of "diff" values, p.52]`:

> For this example, suppose a subscription to a dynamic text, with subscription ID 5, just one item, and the following fields in the schema: `timestamp`, `text`, such as the `text` field has a complex structure that is carried with Json structures.
>
> This is a hypothetical stream of real-time updates and the effect it has on field values on the client. Percent decoding is also needed for some values:

**Diff example 1 — snapshot, no diff** `[§Notification Examples with the use of "diff" values, p.52]`:

```
U,5,1,20:00:33|{ "text": "aa%7Cbb", "attributes": [ { "font": "courier" }, "..." ] }
```

| | timestamp | text |
|---|---|---|
| diff received | | |
| actual value | `20:00:33` | `{ "text": "aa\|bb", "attributes": [ { "font": "courier" }, "..." ] }` |

> The initial update (the snapshot) carries information for all the fields.
> With `"..."` we represent a possibly long snippet in the Json value.

**Diff example 2 — JSON Patch** `[§Notification Examples with the use of "diff" values, p.53]` (the printed line wraps; it is a single logical line):

```
U,5,1,20:00:54|^P[{"op":"replace","path":"/text","value":"aa%7Cbb%7Ccc"},{"op":"add","path":"/attributes/1","value":"bold"}]
```

| | timestamp | text |
|---|---|---|
| diff received | | `[{"op":"replace","path":"/text","value":"aa\|bb\|cc"},{"op":"add","path":"/attributes/1","value":"bold"}]` |
| actual value | `20:00:54` | `{"text":"aa\|bb\|cc","attributes":[{"font":"courier"},"bold","..."]}` |

> This update contains the new values. Since both the previous and the new value of the `text` field happen to be valid JSON representations, the Server chose to provide the field update as a JSON Patch.

Note that the "diff received" row shows the patch **after** percent-decoding (`%7C` → `|`), matching step "take the substring following the letter and decode any percent-encoding" `[§Decoding the Pipe-Separated List of Values, p.51]`.

**Diff example 3 — unchanged** `[§Notification Examples with the use of "diff" values, p.53]`:

```
U,5,1,20:04:16|
```

| | timestamp | text |
|---|---|---|
| diff received | | |
| actual value | `20:04:16` | `{"text":"aa\|bb\|cc","attributes":[{"font":"courier"},"bold","..."]}` |

> In the final update, the `text` field is unchanged; this is carried, as usual, by an empty field.

⚠️ **Spec unclear:** the "actual value" of the `text` field after the JSON Patch is printed as `{"text":"aa|bb|cc","attributes":[{"font":"courier"},"bold","..."]}` — i.e. with the whitespace of the original snapshot value removed `[§Notification Examples with the use of "diff" values, p.53]`. The spec does not state whether the client is expected to re-serialise the JSON document after applying a patch (and if so, in what canonical form) or to apply the patch to the stored text; the two produce different byte sequences for the same logical value `[§Decoding the Pipe-Separated List of Values, p.50]`.

⚠️ **Spec unclear:** the spec states that JSON Patch "can be applied only between values that are valid JSON representations" and guarantees the previous value is valid JSON `[p.50]`, but does not define client behaviour if a received `^P` patch fails to apply (invalid patch, path not found, previous value not parseable) `[§Decoding the Pipe-Separated List of Values, p.50]`.

### 2.7 The `T` diff format — TLCP-diff (Appendix D)

The `T` label's format is defined only in Appendix D `[§Decoding the Pipe-Separated List of Values, p.50]`.

**Nature and preconditions** `[§Appendix D: The custom "TLCP-diff" format, p.98]`:

> The "diff" format used in association with the `T` label, called TLCP-diff, consists in a UTF-8 character sequence that is meant to be applied to a predetermined, base UTF-8 character sequence in order to obtain a result character sequence.
> Note that "TLCP-diff" values, when included in a TLCP update message, may be **percent-encoded**; here we refer to the decoded form.
> Moreover, "TLCP-diff" values included in a TLCP update message obey the restriction that they don't include any character which representation in UTF-16 requires a surrogate pair. Likewise, when a "TLCP-diff" value is included in a TLCP update message, it is ensured that the base value on which it is meant to be applied also obeys the same restriction. This simplifies the application of the "diff" in Java and possibly other languages that internally represent characters in UTF-16.

**Section grammar** `[§Appendix D, p.98]`:

> A character sequence in "TLCP-diff" format is the concatenation of one or more sections, each of which represents an instruction and has one of the following types: COPY, ADD, and DEL. The sequence is such that:
> - The first section is of COPY type.
> - Each non-terminal COPY section is followed by an ADD section.
> - Each non-terminal ADD section is followed by a DEL section.
> - Each non-terminal DEL section is followed by a COPY section.
>
> The different sections have the following form:
> - A COPY section consists in an encoded integer number (possibly 0).
> - An ADD section consists in an encoded integer number (possibly 0) followed by that number of characters.
> - A DEL section consists in an encoded integer number (possibly 0).

So the section type cycle is fixed and positional: `COPY, ADD, DEL, COPY, ADD, DEL, ...`, starting at `COPY`, and the sequence may terminate after any section `[§Appendix D, p.98]`.

**Encoded integer** `[§Appendix D, p.98]`:

> An encoded integer number is a sequence of one or more letters, such that the last one is a small letter whereas any other preceding ones are capital letters.
> This letter sequence forms a radix-26 representation of the number, in which the letters weight progressively. In practice, after converting the letters to lowercase for clarity, each letter weights as its ASCII value minus the ASCII value of 'a'.
>
> This encoding allows parsing the "diff" character sequence into a list of instructions of the form COPY(N), ADD(N, STR), and DEL(N) in a sequential way.

The termination rule is what makes sequential parsing possible: an encoded integer ends at the first **lowercase** letter `[§Appendix D, p.98]`.

**Section-splitting examples**, verbatim `[§Appendix D, p.98]`:

| Value | Sections | List of instructions |
|---|---|---|
| `d` | `d` | COPY(3) |
| `bdzap` | `b` – `dzap` | COPY(1) ADD(3,zap) |
| `bdzapcd` | `b` – `dzap` – `c` – `d` | COPY(1) ADD(3,zap) DEL(2) COPY(3) |
| `adzapad` | `a` – `dzap` – `a` – `d` | COPY(0) ADD(3,zap) DEL(0) COPY(3) |
| `AdacAa` | `Ad` – `a` – `c` – `Aa` | COPY(29) ADD(0,) DEL(2) COPY(26) |

⚠️ **Spec unclear:** the prose definition of the encoded integer and the last row of the table above disagree. Taking the prose literally ("each letter weights as its ASCII value minus the ASCII value of 'a'", radix-26, letters weighting progressively), `Ad` = `a`,`d` = 0,3 evaluates to 0·26 + 3 = **3**, and `Aa` evaluates to **0**; the table states `Ad` = **29** and `Aa` = **26** `[§Appendix D, p.98]`. Both table values are simultaneously satisfied only if each non-final letter contributes `(value + 1)` rather than `value` at its radix weight (i.e. `Aa` → (0+1)·26 + 0 = 26; `Ad` → (0+1)·26 + 3 = 29), which the prose nowhere states. Single-letter integers are unaffected (`d` = 3, `c` = 2, `a` = 0), and every other example in the appendix uses only single-letter integers, so the multi-letter case is attested by exactly one data point. This must be resolved against a live Server before relying on multi-letter encoded integers.

**Application algorithm**, verbatim `[§Appendix D, pp.98–99]`:

> The list of instructions can be applied to the previous value (*base*) to yield the new value (*result*) with the following process:
>
> - set *result* as an empty character sequence
> - set *basePos* as 0
> - For each instruction in the list:
>   - if COPY(N):
>     - append to *result* N characters taken from base starting at position *basePos*
>     - increment *basePos* by N
>   - if ADD(N, STR):
>     - append STR to *result*
>   - If DEL(N):
>     - increment *basePos* by N
> - return *result*
>
> Note that for some instructions N may be 0, hence they are no-ops.

Note that `ADD(N, STR)` does not advance `basePos`, and that positions and counts are expressed in **characters**; because the no-surrogate-pair restriction holds for both the diff and the base `[§Appendix D, p.98]`, characters, UTF-16 code units and Unicode code points coincide for all values that can legally appear.

**Application examples**, verbatim `[§Appendix D, p.99]`:

| Base value | List of instructions | Final value |
|---|---|---|
| `foo` | COPY(3) | `foo` |
| `foobar` | COPY(3) | `foo` |
| `foobar` | COPY(1) ADD(3,zap) | `fzap` |
| `foobar` | COPY(1) ADD(3,zap) DEL(2) COPY(3) | `fzapbar` |
| `foobar` | COPY(0) ADD(3,zap) DEL(0) COPY(3) | `zapfoo` |
| `foobar` | COPY(2) ADD(0,) DEL(2) COPY(2) | `foar` |

The second row shows that a diff that stops early truncates the base — the result is exactly what the instructions produce, with no implicit trailing copy `[§Appendix D, p.99]`.

⚠️ **Spec unclear:** the last application example is not reachable from the section-splitting table: `COPY(2) ADD(0,) DEL(2) COPY(2)` would encode as `cacc`, while the splitting table's fifth row (`AdacAa`) is the only multi-letter case and is not among the application examples `[§Appendix D, pp.98–99]`. No end-to-end example is given that runs a single TLCP-diff string through both tables, and no example of a complete `U` notification carrying a `^T` value appears anywhere in the specification `[§Notification Examples with the use of "diff" values, pp.52–53]`.

⚠️ **Spec unclear:** the spec does not define client behaviour for a malformed TLCP-diff (e.g. an ADD section whose declared length exceeds the remaining characters, or a COPY/DEL that runs past the end of the base value) `[§Appendix D, pp.98–99]`.

---

## 3. Other Subscription-Related Notifications

These notifications provide insights on the status of a specific subscription `[§Other Subscription-Related Notifications, p.53]`.

### 3.1 Successful Subscription — `SUBOK`

**Tag:** `SUBOK` (short of SUBscription OK) `[§Successful Subscription, p.53]`

**Format:** `SUBOK,<subscription-ID>,<num-items>,<num-fields>` `[§Successful Subscription, p.53]`

**Used when:** to notify of a successful subscription (but for those that make use of `COMMAND` mode) `[§Successful Subscription, p.53]`.

**Arguments: 3** `[§Successful Subscription, p.53]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.53]`. |
| 2 | `<num-items>` | numeric | Number of items in the subscription. The number and the order of items in the subscription is determined by the Metadata Adapter `[p.53]`. |
| 3 | `<num-fields>` | numeric | Number of fields in the subscription. The number and the of order of fields in the subscription is determined by the Metadata Adapter `[pp.53–54]`. |

**When sent / client action:** sent after a subscription request has been received and the corresponding subscription has been activated on the Server. After this notification, real-time updates related to the subscription items and fields start to be sent `[§Successful Subscription, p.54]`. The client therefore learns the schema width (`<num-fields>`) it must use when decoding the pipe-separated value list of subsequent `U` notifications for this subscription.

**Notification Example** `[§Successful Subscription, p.54]`:

```
SUBOK,3,1,10
```

### 3.2 Successful Subscription with Command Mode — `SUBCMD`

**Tag:** `SUBCMD` (short of SUBscription in CoMmanD mode) `[§Successful Subscription with Command Mode, p.54]`

**Format:** `SUBCMD,<subscription-ID>,<num-items>,<num-fields>,<key-field>,<command-field>` `[§Successful Subscription with Command Mode, p.54]`

**Used when:** to notify of a successful subscription that makes use of `COMMAND` mode `[§Successful Subscription with Command Mode, p.54]`.

**Arguments: 5** `[§Successful Subscription with Command Mode, p.54]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.54]`. |
| 2 | `<num-items>` | numeric | Number of items in the subscription. The number and the order of items in the subscription is determined by the Metadata Adapter `[p.54]`. |
| 3 | `<num-fields>` | numeric | Number of fields in the subscription. The number and the of order of fields in the subscription is determined by the Metadata Adapter `[p.54]`. |
| 4 | `<key-field>` | numeric, **1-based** | Index of the field that contains the **key**. The order of fields in the subscription is determined by the Metadata Adapter `[p.54]`. |
| 5 | `<command-field>` | numeric, **1-based** | Index of the field that contains the **command**. The order of fields in the subscription is determined by the Metadata Adapter `[p.54]`. |

**When sent / client action:** sent after a subscription request has been received and the corresponding subscription has been activated on the Server. After this notification, real-time updates related to the subscription items and fields start to be sent `[§Successful Subscription with Command Mode, p.54]`. `SUBCMD` replaces `SUBOK` for `COMMAND`-mode subscriptions `[§Successful Subscription, p.53]`; the client must record `<key-field>` and `<command-field>` before decoding any `U` for this subscription, since they identify which decoded field positions carry the key and the command.

**Notification Example** `[§Successful Subscription with Command Mode, p.54]`:

```
SUBCMD,3,1,10,1,2
```

### 3.3 COMMAND-mode semantics visible on the wire

What the specification states about `COMMAND` mode:

- With `COMMAND` mode, the subscription represents a **dynamic table**, where items are rows and fields are columns. Each real-time update **may add a new row, delete an existing row or change the content of a specific row**. A common example is a stock portfolio `[§Subscription Data Model, p.9]`.
- The subscription must have a `key` field and a `command` field in its schema; a subscription to an item in `COMMAND` mode with no `key` field in the schema fails with control error code **15**, and with no `command` field in the schema with control error code **16** `[§Appendix B: Control Error Codes, p.91]`.
- Their positions in the schema are conveyed by `SUBCMD` arguments 4 and 5 `[§Successful Subscription with Command Mode, p.54]`.
- With `COMMAND` mode the snapshot may be composed of multiple `U` notifications, and its end is signalled by `EOS` `[§End of Snapshot, p.55]`.
- With `COMMAND` mode, `CS` (snapshot clearing) is delivered to the client so that it can clear its representation of the item — typically a list or a history with this mode `[§Snapshot Clearing, p.56]`.
- Frequency limits in `COMMAND` mode apply to the **UPDATE commands sent for each key** `[§Subscription Request, p.30]`, `[§Subscription Reconfiguration Request, p.33]`.
- `OV` (overflow) can be sent in `COMMAND` mode **for ADD and DELETE events only**, or for `COMMAND` mode with unfiltered dispatching `[§Overflow, p.56]`.

⚠️ **Spec unclear:** the specification names the commands `ADD`, `DELETE` and `UPDATE` `[§Overflow, p.56]`, `[§Subscription Request, p.30]` and describes their effect on the dynamic table in prose `[§Subscription Data Model, p.9]`, but **nowhere states the literal wire values carried in the command field** of a `U` notification, nor their casing. It also does not state how the key field's value is encoded (whether it is subject to the same `#`/`$`/`^`/percent-encoding rules as any other field — it presumably is, being an ordinary field position), nor what the other field values contain in a `DELETE` event. This document does not fill those gaps.

⚠️ **Spec unclear:** the specification does not state, within chapter 3, the client-side state machine for `COMMAND` mode (e.g. whether an `ADD` for an existing key replaces the row, whether a `DELETE` for an unknown key is an error, or whether unchanged-field markers in a `U` are resolved against the previous update **of the same key** or of the same item) `[§Real-Time Update, pp.49–51]`, `[§Successful Subscription with Command Mode, p.54]`. The decoding algorithm speaks only of "the previous update of the same field" `[p.49]`, without qualification by key.

### 3.4 Successful Unsubscription — `UNSUB`

**Tag:** `UNSUB` (short of UNSUBscription) `[§Successful Unsubscription, p.54]`

**Format:** `UNSUB,<subscription-ID>` `[§Successful Unsubscription, p.54]`

**Used when:** to notify of a successful unsubscription `[§Successful Unsubscription, p.54]`.

**Arguments: 1** `[§Successful Unsubscription, p.55]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.55]`. |

**When sent / client action:** sent after an unsubscription request has been received and the corresponding subscription has been deactivated on the Server. **After this notification, no more real-time updates related to the subscription items and fields will be sent** `[§Successful Unsubscription, p.55]`. The client may therefore discard all state for that subscription ID only once `UNSUB` arrives — updates may still follow the unsubscription request's *response* `[§Basic Subscription, p.15]`.

**Notification Example** `[§Successful Unsubscription, p.55]`:

```
UNSUB,3
```

### 3.5 End of Snapshot — `EOS`

**Tag:** `EOS` (short of End Of Snapshot) `[§End of Snapshot, p.55]`

**Format:** `EOS,<subscription-ID>,<item>` `[§End of Snapshot, p.55]`

**Used when:** to notify the end of the snapshot of an item `[§End of Snapshot, p.55]`.

**Arguments: 2** `[§End of Snapshot, p.55]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.55]`. |
| 2 | `<item>` | numeric, **1-based** | Index of the item whose snapshot has ended. The order of items in the subscription is determined by the Metadata Adapter `[p.55]`. |

**When sent / client action** `[§End of Snapshot, p.55]`:

> This notification is sent immediately after the last `U` notification bringing the snapshot has been sent. The snapshot represents the "initial state" of an item, and may be composed of multiple `U` notifications in case the subscription is in `DISTINCT` or `COMMAND` mode. With `MERGE` mode the snapshot is composed of just one `U` notification and hence this notification is not sent. With `RAW` mode the snapshot is not supported.

The client uses `EOS` as the per-item switch from "snapshot" to "real-time update" classification for `DISTINCT` and `COMMAND` modes `[§Snapshot vs Real-Time Updates, p.51]`.

**Notification Example** `[§End of Snapshot, p.55]`:

```
EOS,3,1
```

### 3.6 Snapshot Clearing — `CS`

**Tag:** `CS` (short of Clear Snapshot) `[§Snapshot Clearing, p.55]`

**Format:** `CS,<subscription-ID>,<item>` `[§Snapshot Clearing, p.55]`

**Used when:** to notify that the snapshot of an item has been cleared `[§Snapshot Clearing, p.55]`.

**Arguments: 2** `[§Snapshot Clearing, p.55]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.55]`. |
| 2 | `<item>` | numeric, **1-based** | Index of the item whose snapshot has been cleared. The order of items in the subscription is determined by the Metadata Adapter `[p.56]`. |

**When sent / client action** `[§Snapshot Clearing, p.56]`:

> This notification is sent when the Data Adapter explicitly asks the Server to clear the snapshot of a subscription. If the subscription is in `MERGE` mode, no notification is sent, as the clearing is handled by the Server by sending an update with null fields. On the other hand, if the subscription is in `DISTINCT` or `COMMAND` mode, the notification is sent to the client so that it can clear its representation of the item (typically a list or a history, with these modes).

**Notification Example** `[§Snapshot Clearing, p.56]`:

```
CS,3,1
```

⚠️ **Spec unclear:** after a `CS`, the spec does not state whether the "previous update of the same field" baseline used by unchanged-field markers and `^`+letter diffs is reset for that item `[§Snapshot Clearing, p.56]`, `[§Decoding the Pipe-Separated List of Values, p.49]`.

### 3.7 Overflow — `OV`

**Tag:** `OV` (short of OVerflow) `[§Overflow, p.56]`

**Format:** `OV,<subscription-ID>,<item>,<overflow-size>` `[§Overflow, p.56]`

**Used when:** to notify that one or more update events for an item have been dropped due to buffer limitations on the Server `[§Overflow, p.56]`.

**Arguments: 3** `[§Overflow, p.56]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.56]`. |
| 2 | `<item>` | numeric, **1-based** | Index of the item whose update events have been cleared. The order of items in the subscription is determined by the Metadata Adapter `[p.56]`. |
| 3 | `<overflow-size>` | numeric | The number of real-time update events that have been dropped `[p.56]`. |

**When sent** `[§Overflow, p.56]`:

> This notification can only be sent if the item was subscribed in `RAW` or `COMMAND` mode (for ADD and DELETE events only), or if it was subscribed in `MERGE`, `DISTINCT`, or `COMMAND` mode (the modes for which events dropping is allowed) and unfiltered dispatching was requested. So, in all these cases, whenever the Server has to drop an event because of resource limits, it notifies the client in this way.

**Notification Example** `[§Overflow, p.56]`:

```
OV,3,1,5
```

⚠️ **Spec unclear:** the parenthetical "(for ADD and DELETE events only)" in the sentence above is positionally ambiguous — it may qualify only the `COMMAND` alternative of the first clause, or the whole first clause including `RAW` `[§Overflow, p.56]`.

### 3.8 Subscription Reconfiguration — `CONF`

**Tag:** `CONF` (short of subscription reCONFiguration) `[§Subscription Reconfiguration, p.56]`

**Format:** `CONF,<subscription-ID>,<max-frequency>,<filtered|unfiltered>` `[§Subscription Reconfiguration, p.56]`

**Used when:** to notify that a subscription has been successfully configured, or reconfigured with a different (or even the same) max frequency `[§Subscription Reconfiguration, p.56]`.

**Arguments: 3** `[§Subscription Reconfiguration, p.56]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<subscription-ID>` | numeric | The ID of the subscription `[p.57]`. |
| 2 | `<max-frequency>` | decimal number, or the literal `unlimited` | New maximum frequency of the subscription, expressed in updates/sec as a decimal number. The value `unlimited` is also possible, meaning that no limit is applied on the frequency `[p.57]`. |
| 3 | `<filtered\|unfiltered>` | literal `filtered` or `unfiltered` | Filtered/unfiltered flag for the subscription `[p.57]`. |

Notes attached to `<max-frequency>`, verbatim `[§Subscription Reconfiguration, p.57]`:

> - This value reports the maximum frequency for all the items in the subscription, considering that different items may have different maximum frequencies (as the Metadata Adapter can limit the frequency on an item by item basis).
> - The computed maximum frequency may be an approximation of the requested maximum frequency, due to the Server internal handling of update scheduling.

**When sent / client action** `[§Subscription Reconfiguration, p.57]`:

> This notification is sent at the start of a subscription and after a subscription has been reconfigured following a Subscription Reconfiguration Request (see Subscription Reconfiguration Request). After this notification, real-time updates are sent with the specified maximum frequency.

**Notification Example** `[§Subscription Reconfiguration, p.57]`:

```
CONF,3,3.0,filtered
```

⚠️ **Spec unclear:** the exact lexical form of the decimal `<max-frequency>` is not specified for the notification (the corresponding *request* parameter is specified as using a dot as decimal separator `[§Subscription Request, p.30]`, and the example shows `3.0`, but the notification section itself only says "a decimal number" `[§Subscription Reconfiguration, p.57]`).

---

## 4. Message-Related Notifications

These notifications provide insights on the status of a specific upstream message `[§Message-Related Notifications, p.59]`.

### 4.1 Message Successfully Sent — `MSGDONE`

**Tag:** `MSGDONE` (short of MeSsaGe send DONE) `[§Message Successfully Sent, p.60]`

**Format:** `MSGDONE,<sequence>,<prog>,<response>` `[§Message Successfully Sent, p.60]`

**Used when:** to notify that a message has been successfully sent and processed `[§Message Successfully Sent, p.60]`.

**Arguments: 3** `[§Message Successfully Sent, p.60]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<sequence>` | string, or the literal `*` | Sequence identifier specified in the send request, or `*`, if a sequence had not been specified `[p.60]`. |
| 2 | `<prog>` | numeric | Message progressive number specified in the send request `[p.60]`. |
| 3 | `<response>` | string (may be empty) | Response message from the Metadata Adapter. If not supplied (i.e. supplied as null), an **empty** message is received here `[p.60]`. |

**When sent / client action** `[§Message Successfully Sent, p.60]`:

> This notification is sent after the corresponding message has been successfully processed. A message is considered processed when the Metadata Adapter has received it in its `notifyUserMessage` event and returned with no exceptions. See the Metadata Adapter SDK API Reference for more information.

Being non-numeric, `<sequence>` and `<response>` are percent-decoded in the first parsing pass `[§Common Response and Notification Syntax, p.14]`; `<response>` is the last argument and may itself contain commas `[§Common Response and Notification Syntax, p.14]`.

**Notification Example** `[§Message Successfully Sent, p.60]`:

```
MSGDONE,Orders_Sequence,3,Processed with ID 32652506
```

### 4.2 Message Send Failed — `MSGFAIL`

**Tag:** `MSGFAIL` (short of MeSsaGe send FAILed) `[§Message Send Failed, p.60]`

**Format:** `MSGFAIL,<sequence>,<prog>,<error-code>,<error-message>` `[§Message Send Failed, p.60]`

**Used when:** to notify that a message has not been sent or its processing failed `[§Message Send Failed, p.60]`.

**Arguments: 4** `[§Message Send Failed, p.60]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<sequence>` | string, or the literal `*` | Sequence identifier specified in the send request, or `*`, if a sequence had not been specified `[p.60]`. |
| 2 | `<prog>` | numeric | Message progressive number specified in the send request `[p.60]`. |
| 3 | `<error-code>` | numeric | Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see Appendix B `[p.60]`. |
| 4 | `<error-message>` | string (may be empty) | Error message sent by Server kernel or by the Metadata Adapter. Note that a message sent by the Metadata Adapter may be empty `[p.60]`. |

**When sent** `[§Message Send Failed, p.61]`:

> This notification is sent when the corresponding message has not been received by the Server or its processing failed for some reason.

**Notification Example** `[§Message Send Failed, p.61]`:

```
MSGFAIL,Orders_Sequence,4,38,The specified progressive number has been skipped by timeout
```

---

## 5. Session-Related Notifications

These notifications provide insights on the status of the current session `[§Session-Related Notifications, p.61]`.

### 5.1 Session Constraints Changed — `CONS`

**Tag:** `CONS` (short of CONStrain) `[§Session Constraints Changed, p.61]`

**Format:** `CONS,<bandwidth>` `[§Session Constraints Changed, p.61]`

**Used when:** to notify that a session has been successfully configured, or reconfigured with a different (or even the same) maximum bandwidth `[§Session Constraints Changed, p.61]`.

**Arguments: 1** `[§Session Constraints Changed, p.61]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<bandwidth>` | decimal number, or the literal `unlimited`, or the literal `unmanaged` | New maximum bandwidth of the session, expressed in **kbps** as a decimal number. The value `unlimited` is also possible, meaning that no limit is applied on the bandwidth. The value `unmanaged` is possible as well; it is equivalent to `unlimited`, but it also notifies that **the client is not allowed to limit the bandwidth for this session** `[p.61]`. |

**When sent / client action** `[§Session Constraints Changed, p.61]`:

> This notification is sent at the beginning of the session and after the session constraints, in particular its maximum bandwidth, has been changed following a Session Constrain Request (see Session Constrain Request). The bandwidth may be changed also by server-side actions. After this notification, real-time updates are sent within the specified maximum bandwidth limit.

**Notification Example** `[§Session Constraints Changed, p.61]`:

```
CONS,50
```

### 5.2 Time Synchronization — `SYNC`

**Tag:** `SYNC` (short of SYNChronism) `[§Time Synchronization, p.61]`

**Format:** `SYNC,<seconds-since-initial-header>` `[§Time Synchronization, p.61]`

**Used when:** to notify the time elapsed on the Server since the session was bound `[§Time Synchronization, p.61]`.

**Arguments: 1** `[§Time Synchronization, p.61]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<seconds-since-initial-header>` | numeric (seconds) | Time elapsed on the Server since the session was bound, expressed in seconds `[p.61]`. |

**When sent / client action** `[§Time Synchronization, p.61]`:

> This notification is sent periodically to notify the client of the time elapsed on the Server. Official Lightstreamer client libraries exploit this notification to detect when they are not keeping up with the current flow of real-time updates. **The notifications are not sent on polling connections.**

**Notification Example** `[§Time Synchronization, p.62]`:

```
SYNC,120
```

⚠️ **Spec unclear:** the period at which `SYNC` is sent is not specified, and no tolerance is given for the "not keeping up" comparison `[§Time Synchronization, p.61]`.

### 5.3 Client IP Address — `CLIENTIP`

**Tag:** `CLIENTIP` (short of CLIENT IP address) `[§Client IP Address, p.62]`

**Format:** `CLIENTIP,<client-IP>` `[§Client IP Address, p.62]`

**Used when:** to notify the IP address of the client, as seen by the Server `[§Client IP Address, p.62]`.

**Arguments: 1** `[§Client IP Address, p.62]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<client-IP>` | string (IPv4 or IPv6 textual address) | IP address of the client, as seen by the Server `[p.62]`. |

**When sent / client action** `[§Client IP Address, p.62]`:

> This notification is sent to notify the client of its own IP address, as it appears on the Server.
> Note that this value, in principle, may be different for each request, even though related to the same Session; hence it is **reissued at each bind, unless unchanged**.
> Official Lightstreamer client libraries exploit this notification to detect when the client has changed its network (e.g. it passed from a 3G WAN to a local Wi-Fi), so that it can try again to reconnect with a transport that previously failed. Note that this notification **may be disabled** with the Server configuration element `<client_identification>`, which should be set consistently on all ports.

**Notification Example with IPv4 address** `[§Client IP Address, p.62]`:

```
CLIENTIP,127.0.0.1
```

**Notification Example with IPv6 address** `[§Client IP Address, p.62]`:

```
CLIENTIP,::1
```

Note that the IPv6 form contains colons but no commas, so it survives the comma-splitting first pass unchanged `[§Common Response and Notification Syntax, p.14]`.

### 5.4 Server Name — `SERVNAME`

**Tag:** `SERVNAME` (short of SERVer NAME) `[§Server Name, p.62]`

**Format:** `SERVNAME,<server-name>` `[§Server Name, p.62]`

**Used when:** to notify the Server configured name `[§Server Name, p.62]`.

**Arguments: 1** `[§Server Name, p.62]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<server-name>` | string | The configured name of the Server `[p.62]`. |

**When sent / client action** `[§Server Name, p.62]`:

> This notification is sent to let the client know the configured name of the Server (i.e. the "name" attribute specified for the listening port in Lightstreamer Server configuration).
> Note that this value, in principle, may be different for each request, even though related to the same Session; hence it is **reissued at each bind, unless unchanged**.
> Official Lightstreamer client libraries provide this information to the application, for example to improve diagnostics.

**Notification Example** `[§Server Name, p.62]`:

```
SERVNAME,Lightstreamer HTTP Server
```

### 5.5 Progressive of Last Data Notification — `PROG`

**Tag:** `PROG` (short of PROGressive) `[§Progressive of Last Data Notification, p.63]`

**Format:** `PROG,<progressive>` `[§Progressive of Last Data Notification, p.63]`

**Used when:** to notify the starting point of the response flow `[§Progressive of Last Data Notification, p.63]`.

**Arguments: 1** `[§Progressive of Last Data Notification, p.63]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<progressive>` | numeric | The progressive number of the last data notification already sent (or, equivalently, the total count of data notifications already sent); the response flow will start from this point `[p.63]`. |

**When sent / client action** `[§Progressive of Last Data Notification, p.63]`:

> This notification is sent only when requested via `LS_recovery_from` on bind_session requests. The specified progressive should be the same as the one specified through `LS_recovery_from`, but actually it can be **lower**. In this case, the initial data notifications received must be **skipped**, until the required progressive number is reached. However, **any notification of different kind received must always be obeyed**.

The set of notifications counted as "**data notifications**" is defined as those related with the subscription and message activity, i.e. those reported in the *Real-Time Update*, *Other Subscription-Related Notifications* and *Message-Related Notifications* sections `[§Session Recovery, p.8]`. By that definition the counted tags are: `U`, `SUBOK`, `SUBCMD`, `UNSUB`, `EOS`, `CS`, `OV`, `CONF`, `MSGDONE`, `MSGFAIL`. All other notifications "must always be treated as usual: they are not related with data, but only with the session and connection lifecycle and are also not meant to be resumed" `[§Session Recovery, p.8]`.

**Notification Example** `[§Progressive of Last Data Notification, p.63]`:

```
PROG,24576
```

⚠️ **Spec unclear:** the "data notification" definition is by reference to three whole document sections `[§Session Recovery, p.8]`; the *MPN-Related Notifications* section is not among them, but the spec does not say explicitly that MPN notifications are excluded from the count. The enumeration of counted tags above is derived from the section membership, not stated tag-by-tag anywhere in the spec.

### 5.6 No Operation — `NOOP`

**Tag:** `NOOP` (short of NO OPeration) `[§No Operation, p.63]`

**Format:** `NOOP,<preamble>` `[§No Operation, p.63]`

**Used when:** to send dummy content to the client `[§No Operation, p.63]`.

**Arguments: 1** `[§No Operation, p.63]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<preamble>` | string | Dummy content to be **ignored** `[p.63]`. |

**When sent / client action** `[§No Operation, p.63]`:

> The purpose of this notification is to fill the receive buffer of the client browser or the operating system during the initial setup phase of the session. While this may seem weird, consider that certain operating systems buffer the content of an HTTP response until some specific length (e.g. 2 Kbytes). Preemptively filling this buffer lets the client receive subsequent content as soon as it is sent by the Server.

**Notification Example** `[§No Operation, p.63]`:

```
NOOP,sending placeholder data
```

### 5.7 Probe (Keep-Alive) — `PROBE`

**Tag:** `PROBE` `[§Probe (Keep-Alive), p.63]`

**Format:** `PROBE` `[§Probe (Keep-Alive), p.63]`

**Used when:** to keep the stream connection alive `[§Probe (Keep-Alive), p.64]`.

**Arguments: 0** `[§Probe (Keep-Alive), p.64]`

`PROBE` is therefore a bare tag with no comma at all — the first-pass parser's "look for the first comma (it may not be there)" branch `[§Common Response and Notification Syntax, p.14]`.

**When sent / client action** `[§Probe (Keep-Alive), p.64]`:

> This notification is sent periodically by the Server when no other activity has been sent on the stream connection. The interval is specified in the Server configuration, but it may be changed during session creation; the actual interval is reported in the session creation response. **The notifications are not sent on polling connections.**
>
> Official Lightstreamer client libraries monitor this notification to detect when the connection is stalled: if after the expected interval plus a configurable timeout no `PROBE` has been received, the connection is closed and reopened.

**Notification Example** `[§Probe (Keep-Alive), p.64]`:

```
PROBE
```

### 5.8 Stream Connection Loop — `LOOP`

**Tag:** `LOOP` `[§Stream Connection Loop, p.64]`

**Format:** `LOOP,<expected-delay>` `[§Stream Connection Loop, p.64]`

**Used when:** to signal that the session needs to be rebound `[§Stream Connection Loop, p.64]`.

**Arguments: 1** `[§Stream Connection Loop, p.64]`

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<expected-delay>` | numeric (milliseconds) | Expected delay before rebinding, expressed in milliseconds `[p.64]`. |

Notes on `<expected-delay>`, verbatim `[§Stream Connection Loop, p.64]`:

> - A value of 0 means that the client should rebind the session as soon as possible.
> - A value greater than 0 is actually used only on polling sessions requested with `LS_polling_millis` > 0, i.e. synchronous polling. In this situation, the expected-delay may be lower than originally requested with `LS_polling_millis`.

**When sent / client action** `[§Stream Connection Loop, p.64]`:

> Reasons for a session to be rebound are mainly the following:
> - The polling cycle is complete.
> - The Content-Length of an HTTP stream connection has been reached.
> - A rebind has been explicitly asked by the client.
>
> In all these situations the session is unbound and a `LOOP` notification is sent. The client is expected to react by **rebinding the session** (see Session Binding Request).
>
> Note: since this notification marks the end of the stream connection, with HTTP transport, any spurious character following the CR-LF of this notification may be safely ignored.

**Notification Example** `[§Stream Connection Loop, p.64]`:

```
LOOP,5000
```

### 5.9 Stream Connection End — `END`

**Tag:** `END` `[§Stream Connection End, p.64]`

**Format:** `END,<cause-code>,<cause-message>` `[§Stream Connection End, p.65]`

**Used when:** to signal that the session has been closed by the Server `[§Stream Connection End, p.65]`.

**Arguments:** the spec prints "**Arguments: 1**" and then lists **two** arguments `[§Stream Connection End, p.65]`.

| # | Name | Type | Meaning |
|---|---|---|---|
| 1 | `<cause-code>` | numeric | Cause code sent by the Server kernel. For a list of supported cause codes see Appendix A `[p.65]`. |
| 2 | `<cause-message>` | string | Short cause description sent by the Server kernel `[p.65]`. |

⚠️ **Spec unclear:** the argument count for `END` is printed as **1** while two arguments are documented, and the `Format` line, the base-syntax example list `[§Common Response and Notification Syntax, p.14]` and the notification example all show two `[§Stream Connection End, p.65]`. The count line is presumed to be a typo, but the spec is self-contradictory as written; note that "each different tag has a fixed number of arguments" is the premise of the whole parsing algorithm `[§Common Response and Notification Syntax, p.13]`.

**When sent / client action** `[§Stream Connection End, p.65]`:

> This notification is sent when the Server closes the session for some reason, e.g. on request by an administrator or as a consequence of a Session Destroy request.
>
> Note: since this notification marks the end of the stream connection, with HTTP transport, any spurious character following the CR-LF of this notification may be safely ignored.

**Notification Example** `[§Stream Connection End, p.65]`:

```
END,8,Session count limit reached
```

---

## 6. Tag Index

All notification tags documented in this chapter, with their fixed argument count as printed by the spec.

| Tag | Args | Notification | Section | Data notification? |
|---|---|---|---|---|
| `U` | 3 | Real-Time Update `[p.49]` | §2 | yes `[§Session Recovery, p.8]` |
| `SUBOK` | 3 | Successful Subscription `[p.53]` | §3.1 | yes `[p.8]` |
| `SUBCMD` | 5 | Successful Subscription with Command Mode `[p.54]` | §3.2 | yes `[p.8]` |
| `UNSUB` | 1 | Successful Unsubscription `[p.54]` | §3.4 | yes `[p.8]` |
| `EOS` | 2 | End of Snapshot `[p.55]` | §3.5 | yes `[p.8]` |
| `CS` | 2 | Snapshot Clearing `[p.55]` | §3.6 | yes `[p.8]` |
| `OV` | 3 | Overflow `[p.56]` | §3.7 | yes `[p.8]` |
| `CONF` | 3 | Subscription Reconfiguration `[p.56]` | §3.8 | yes `[p.8]` |
| `MSGDONE` | 3 | Message Successfully Sent `[p.60]` | §4.1 | yes `[p.8]` |
| `MSGFAIL` | 4 | Message Send Failed `[p.60]` | §4.2 | yes `[p.8]` |
| `CONS` | 1 | Session Constraints Changed `[p.61]` | §5.1 | no `[p.8]` |
| `SYNC` | 1 | Time Synchronization `[p.61]` | §5.2 | no `[p.8]` |
| `CLIENTIP` | 1 | Client IP Address `[p.62]` | §5.3 | no `[p.8]` |
| `SERVNAME` | 1 | Server Name `[p.62]` | §5.4 | no `[p.8]` |
| `PROG` | 1 | Progressive of Last Data Notification `[p.63]` | §5.5 | no `[p.8]` |
| `NOOP` | 1 | No Operation `[p.63]` | §5.6 | no `[p.8]` |
| `PROBE` | 0 | Probe (Keep-Alive) `[p.64]` | §5.7 | no `[p.8]` |
| `LOOP` | 1 | Stream Connection Loop `[p.64]` | §5.8 | no `[p.8]` |
| `END` | 1 *(two documented — see §5.9)* | Stream Connection End `[p.65]` | §5.9 | no `[p.8]` |

Not covered here — see the MPN chapter: `MPNREG`, `MPNZERO`, `MPNOK`, `MPNCONF`, `MPNDEL` `[§MPN-Related Notifications, pp.57–59]`.

Note that the spec's claim that "the first 4 characters of a tag are unique" `[§Common Response and Notification Syntax, p.14]` holds across this set only when the response tags of chapter 2 are also considered; within this chapter, `SUBO`/`SUBC` and `MSGD`/`MSGF` are the closest pairs and remain distinct.

---

## 7. Index of flagged ambiguities

1. §2.3 — value list shorter than the schema, and `^N` overrunning the schema, are undefined `[pp.50–51]`.
2. §2.3 — `^` followed by neither letter nor digit, and unknown diff letters, are undefined `[p.50]`.
3. §2.3 — lexical form of the integer after `^` is unconstrained `[p.50]`.
4. §2.3 — the set of percent-encoded meta characters inside the value list is given only by example `[p.50]`.
5. §2.3 — applying a diff when the pointed field is currently *empty*, or never yet set, is undefined `[p.50]`.
6. §2.5 — Example 5 (`U,3,1,20:06:10|3.05|0.32|^7`) contradicts its own result table `[p.52]`.
7. §2.6 — JSON re-serialisation after a JSON Patch is unspecified `[p.53]`.
8. §2.6 — behaviour on a failed JSON Patch application is unspecified `[p.50]`.
9. §2.7 — the encoded-integer prose contradicts the `Ad`=29 / `Aa`=26 table entries `[p.98]`.
10. §2.7 — no end-to-end TLCP-diff example, and no `^T` example on the wire `[pp.52–53, 98–99]`.
11. §2.7 — behaviour on a malformed TLCP-diff is unspecified `[pp.98–99]`.
12. §3.3 — literal wire values of the `COMMAND` command field are never stated `[p.56, p.9]`.
13. §3.3 — the `COMMAND`-mode client-side state machine, and whether unchanged-field baselines are per-key, are not stated `[pp.49–51, 54]`.
14. §3.6 — whether `CS` resets the unchanged-field baseline is unstated `[p.56]`.
15. §3.7 — the "(for ADD and DELETE events only)" parenthetical in `OV` is positionally ambiguous `[p.56]`.
16. §3.8 — lexical form of `CONF`'s decimal `<max-frequency>` is unspecified `[p.57]`.
17. §5.2 — `SYNC` period and staleness tolerance are unspecified `[p.61]`.
18. §5.5 — the tag-level membership of "data notifications" is derived, not enumerated `[p.8]`.
19. §5.9 — `END` is printed with "Arguments: 1" but documents two `[p.65]`.
