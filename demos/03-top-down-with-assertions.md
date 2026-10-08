# Demo 3: Writing a proof top-down with assertions

State the lemmas a proof needs as unchecked assertions (`a` lines),
check that the conclusion follows from them, then replace each
assertion with a real derivation. The REPL shows which constraints are
relevant to each assertion as you replace it.

**Files:** [`files/pigeons.opb`](files/pigeons.opb)
**Run from:** the repository root, with `veripb` configured as in
[README.md](../README.md).

## The problem

`pigeons.opb` is the pigeonhole principle for 3 pigeons and 2 holes;
`p<i><j>` means pigeon `i` is in hole `j`:

```
* every pigeon is in a hole
@p1_placed 1 p11 1 p12 >= 1 ;
@p2_placed 1 p21 1 p22 >= 1 ;
@p3_placed 1 p31 1 p32 >= 1 ;
* no two pigeons share hole 1
@h1_not_12 1 ~p11 1 ~p21 >= 1 ;
@h1_not_13 1 ~p11 1 ~p31 >= 1 ;
@h1_not_23 1 ~p21 1 ~p31 >= 1 ;
* no two pigeons share hole 2
@h2_not_12 1 ~p12 1 ~p22 >= 1 ;
@h2_not_13 1 ~p12 1 ~p32 >= 1 ;
@h2_not_23 1 ~p22 1 ~p32 >= 1 ;
```

Plan: show that each hole holds at most one pigeon
(`~p1j + ~p2j + ~p3j >= 2`); adding those two facts to the three
`@p<i>_placed` constraints gives `0 >= 1`.

## 1. Sketch the proof

```
cargo run -- demos/files/pigeons.opb
```

Type the two lemmas as labelled assertions, then the final step that
uses them:

```
@h1_amo a 1 ~p11 1 ~p21 1 ~p31 >= 2 ;
@h2_amo a 1 ~p12 1 ~p22 1 ~p32 >= 2 ;
@contradiction pol @p1_placed @p2_placed + @p3_placed + @h1_amo + @h2_amo + ;
:check
```

Each line is checked as you type it; an `a` line is accepted without a
derivation. The assertions are shown on a red background in the Proof
pane. `@contradiction` is `>= 1`, and `:check` reports
`s UNDER ASSERTIONS UNSATISFIABLE`: the conclusion follows, provided the
assertions hold.

![The sketch: two assertions and the final step](img/03-sketch.svg)

## 2. Replace the first assertion

```
:deassert
```

`:deassert` opens Vim Insert mode at the start of the first `a` line. It
lists the database constraints that share variables with the assertion,
most shared variables first, and highlights them in the Database pane.
Here `@h1_not_12`, `@h1_not_13` and `@h1_not_23` are the ingredients:
summing them gives `2 ~p11 2 ~p21 2 ~p31 >= 3`, and dividing by 2 rounds
up to `>= 2`.

![:deassert on the first assertion](img/03-deassert.svg)

Type the derivation, keeping the label `@h1_amo` so the final step still
finds it, and press Enter, which splits the line: the new text stays on
line 3 and the old assertion moves to line 4. Press Esc, then `dd` to
delete the old assertion, then Esc to leave Vim mode:

```
@h1_amo pol @h1_not_12 @h1_not_13 + @h1_not_23 + 2 d ;
```

```
:verify
```

The new line derives the same constraint as the assertion did, so the
later lines still check. One assertion remains:

![One assertion left](img/03-one-left.svg)

## 3. Replace the second assertion and conclude

```
:deassert
```

`:deassert` now opens the remaining `a` line. Replace it the same way
with the hole-2 constraints:

```
@h2_amo pol @h2_not_12 @h2_not_13 + @h2_not_23 + 2 d ;
```

```
:verify
:check
```

No assertions remain, and `:check` reports `s VERIFIED UNSATISFIABLE`.

![The completed proof](img/03-proved.svg)

## Commands used

| Command | Purpose |
|---|---|
| `a <constraint> ;` | Add a constraint without a derivation (an assertion). |
| `:check` | Test for a conclusion; `UNDER ASSERTIONS` while assertions remain. |
| `:deassert [n]` | Replace the first `a` line, or line `n`, with derivation steps; lists and highlights related constraints. |
| `:verify` | Check unchecked lines. |
