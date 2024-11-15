# Contributors Guide

Anchor is an open-source Secret Shared Validator (SSV) client. We're community driven and
welcome all contribution. We aim to provide a constructive, respectful and fun
environment for collaboration.

This guide is geared towards beginners. If you're an open-source veteran feel
free to just skim this document and get straight into crushing issues.

## Why Contribute

There are many reasons you might contribute to Anchor. For example, you may
wish to:

- contribute to the SSV ecosystem.
- work on low-level blockchain protocol problems
- contribute to the Ethereum ecosystem.
- work in the amazing Rust programming language.
- learn how to participate in open-source projects.
- expand your software development skills.
- flex your skills in a public forum to expand your career
  opportunities (or simply for the fun of it).

## How to Contribute

Regardless of the reason, the process to begin contributing is very much the
same. We operate like a typical open-source project operating on GitHub: the
repository [Issues](https://github.com/sigp/anchor/issues) is where we
track what needs to be done and [Pull
Requests](https://github.com/sigp/anchor/pulls) is where code gets
reviewed. We use [discord](TODO) to chat
informally.

### General Work-Flow

We recommend the following work-flow for contributors:

1. **Find an issue** to work on, either because it's interesting or suitable to
   your skill-set. Use comments to communicate your intentions and ask
   questions.
2. **Work in a feature branch** of your personal fork
   (github.com/YOUR_NAME/anchor) of the main repository
   (github.com/sigp/anchor).
3. Once you feel you have addressed the issue, **create a pull-request** with
   `unstable` as the base branch to merge your changes into the main repository.
4. Wait for the repository maintainers to **review your changes** to ensure the
   issue is addressed satisfactorily. Optionally, mention your PR on
   [discord](TODO).
5. If the issue is addressed the repository maintainers will **merge your
   pull-request** and you'll be an official contributor!

Generally, you find an issue you'd like to work on and announce your intentions
to start work in a comment on the issue. Then, do your work on a separate
branch (a "feature branch") in your own fork of the main repository. Once
you're happy and you think the issue has been addressed, create a pull request
into the main repository.

### First-time Set-up

First time contributors can get their git environment up and running with these
steps:

1. [Create a
   fork](https://help.github.com/articles/fork-a-repo/#fork-an-example-repository)
   and [clone
   it](https://help.github.com/articles/fork-a-repo/#step-2-create-a-local-clone-of-your-fork)
   to your local machine.
2. [Add an _"upstream"_
   branch](https://help.github.com/articles/fork-a-repo/#step-3-configure-git-to-sync-your-fork-with-the-original-spoon-knife-repository)
   that tracks github.com/sigp/anchor using `$ git remote add upstream
   https://github.com/sigp/anchor.git` (
   pro-tip: [use SSH](https://help.github.com/articles/connecting-to-github-with-ssh/) instead of HTTPS).
3. Create a new feature branch with `$ git checkout -b your_feature_name`. The
   name of your branch isn't critical but it should be short and instructive.
   E.g., if you're fixing a bug with serialization, you could name your branch
   `fix_serialization_bug`.
4. Make sure you sign your commits.
   See [relevant doc](https://help.github.com/en/github/authenticating-to-github/about-commit-signature-verification).
5. Commit your changes and push them to your fork with `$ git push origin
   your_feature_name`.
6. Go to your fork on github.com and use the web interface to create a pull
   request into the sigp/anchor repo.

From there, the repository maintainers will review the PR and either accept it
or provide some constructive feedback.

There's a great
[guide](https://akrabat.com/the-beginners-guide-to-contributing-to-a-github-project/)
by Rob Allen that provides much more detail on each of these steps, if you're
having trouble. As always, jump on [discord](https://discord.gg/cyAszAh)
if you get stuck.

Additionally,
the ["Contributing to Anchor" section](https://anchor-book.sigmaprime.io/contributing.html#contributing-to-anchor)
of the Anchor Book provides more details on the setup.

## FAQs

### I don't think I have anything to add

There's lots to be done and there's all sorts of tasks. You can do anything
from enhancing documentation through to writing core consensus code. If you reach out,
we'll include you.

Please note, to maintain project quality, we may not accept PRs for small typos or changes
with minimal impact.

### I'm not sure my Rust is good enough

We're open to developers of all levels. If you create a PR and your code
doesn't meet our standards, we'll help you fix it and we'll share the reasoning
with you. Contributing to open-source is a great way to learn.

### I'm not sure I know enough about Anchor

No problems, there's plenty of tasks that don't require extensive Anchor
knowledge. You can learn about Anchor as you go.

### I'm afraid of making a mistake and looking silly

Don't be. We're all about personal development and constructive feedback. If you
make a mistake and learn from it, everyone wins.

### I don't like the way you do things

Please, make an issue and explain why. We're open to constructive criticism and
will happily change our ways.
