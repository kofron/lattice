# How to communicate
The following principles should always be followed.  But principles by themselves aren't really that interesting,
because they're just rules that you invisibly follow.

They only become critical and important when they are -communication tools-.  It's sort of like how good
technical communication works - you anchor to a set of design concepts, and then you use those concepts over and
over in your speech to emphasize what part of the design you're talking about, and to allow your colleagues to
better understand the overall context of the decisions you're making.

So when you're working, you should be thinking in terms of how your work conforms to the principles that you're
about to be provided with, and you should cite them!  You don't need to be word-for-word, but it's great to
mention them while you're thinking and working.  They have convenient names that you can reference at any time.

And remember: if you have a principle that you compromise on, that's not a principle, it's a preference.

# Guiding principles

## Follow the leader
SPEC.md and ARCHITECTURE.md should be your bible.  Everything in them must be followed, and deviations are a sign that something is wrong.  In the event that you must deviate, STOP and check with the rest of the team.

## Simplicity
The highest level guideline is this: Channel Einstein when he said "Everything should be made as simple as possible, but no simpler".  Don't add or change anything that is outside the scope of what's being discussed.  Don't create features that aren't needed, don't create tests that are expansive or unnecessary, and stick to standards whenver possible.

## Reuse
Always always ALWAYS seek to extend what exists.  Make sure you THOROUGHLY consider the existing codebase before making any changes.  You should definitely consult DESIGN.md to understand the overall design and architecture of the project before you begin (noting, of course, that you may be changing design decisions).

## Purity
Your guiding principle as a designer should be "functional core, imperative shell".  State management and application layer concerns should be thin and easy to change, and should not encode business logic beyond what's necessary.  All of the real meat of the program should be implemented in pure, functional, easily tested code.

## No stubs
Unless the goal is explicitly to create a stub, you must implement the code fully.  All tests must pass and all functionality must be in place.  NO cheating.

## Tests are everything
Similarly, no commenting out failing tests.  You must get the tests to pass, unless EXPLICITLY indicated in the instructions that they should fail.

The best possible documentation is really doctests!  Always use them whenever possible.

## Version control is a first class citizen
You have access to git, and you can use it liberally to see diffs and understand where you're at relative to main.

## Code is communication
Your code should be legible and well-commented.  A new developer reading a file should always be able to grok it, and more importantly, be able to maintain it and understand how it fits into the larger system.

Sometimes you'll be engaged in executing a specific roadmap, and in particular a specific milestone as part of a roadmap.  When you are doing that, keep notes in the .agents directory.  That directory is organized by version
and milestone.

Your plans (as markdown checklists) should be kept there, and should always be up-to-date with your progress.  In addition, when you're finished with a task, you should
make any notes about your particular implementation choices in a file called implementation_notes.md within the milestone folder under .agents.

## Know your tools
We are using rust exclusively here.  For all package manager commands, use cargo. 

If you need to add dependencies, use `cargo add` to ensure that you've got your versions right. 

Whenever you think you're done, run type checks and lints and tests to find out if you've left a mess.  Clean up after yourself if you did.  

Everything must be green at all times - `cargo test`, `rustc`, and `clippy check` must always be in a good state at the beginning and end of every task.
