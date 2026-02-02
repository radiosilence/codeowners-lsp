_I made a thing: codeowners-lsp_ :sparkles:

So I got tired of the CODEOWNERS experience being... well, terrible. You know the drill:

You add a pattern, CI breaks, and you realize you typo'd the glob pattern or forgot to add your new code. Or you spend 10 minutes scrolling through 3000 lines trying to figure out which rule actually applies to a file. Or there's 47 rules for a team that got renamed 6 months ago.

_codeowners-lsp_ fixes all of this by giving CODEOWNERS files actual IDE support. It can even handle the 3k+ lines of shedul-umbrella without breaking a sweat :crab:

_What does it actually do?_

When you're editing a CODEOWNERS file:

:red*circle: *Real-time error checking* - patterns that match zero files light up red \_as you type*. No more shipping broken rules.

:ghost: _Dead rule detection_ - that rule on line 847 that's completely shadowed by the `*` on line 12? Now you'll know.

:busts_in_silhouette: _GitHub validation_ - hover over `@some-team` and see if they actually exist, who's in them, what they do. Autocomplete suggests real teams from your org.

:compass: _Navigation_ - hover any file in your codebase to see who owns it. Click to jump straight to the CODEOWNERS rule.

And when you're working in _any_ file:

:rotating_light: _"File not owned" errors_ - if a file isn't covered by any CODEOWNERS rule, you'll see a full-file error. Impossible to miss.

:muscle: _Take ownership actions_ - Cmd+. → "Take ownership as @myteam" → done. It figures out the right place to insert the rule automatically.

_Getting started_

_Zed_ :zap:
Install the dev extension from <https://github.com/radiosilence/codeowners-zed|radiosilence/codeowners-zed> - not yet in the marketplace.

_VSCode_
Install the dev extension from <https://github.com/radiosilence/codeowners-vscode|radiosilence/codeowners-vscode> - not yet in the marketplace.

_Optional: GitHub integration_

To get team/user validation and autocomplete, create `.codeowners-lsp.toml` in your repo:

```
github_token = "env:GITHUB_TOKEN"
validate_owners = true
```

Now hovering over `@shedul/payments` shows you the team description and member count :eyes:

_There's a CLI too_

Also ships with `codeowners-cli` for CI and scripts:

```
mise use -g github:radiosilence/codeowners-lsp@latest
```

```
codeowners-cli lint --json        # CI-friendly linting
codeowners-cli check src/foo.ts   # Who owns this?
codeowners-cli coverage           # What's not covered?
codeowners-cli tree               # Visualize ownership
```

_Links_

:github: <https://github.com/radiosilence/codeowners-lsp|radiosilence/codeowners-lsp>
:zap: <https://github.com/radiosilence/codeowners-zed|Zed extension>

Let me know if you hit any issues or have ideas! :pray:
