# codeowners-lsp

Language server providing CODEOWNERS information via hover and inlay hints.

## Features

- **Hover**: Shows file ownership when hovering over any code
- **Inlay Hints**: Displays ownership at the top of each file
- **Code Actions**: Take ownership of files directly from your editor
  - Take ownership as individual (configured owner)
  - Take ownership as team (configured owner)
  - Take ownership as custom (manual entry)
  - Add to existing CODEOWNERS entry or create new specific entry

## Installation

Download the latest release for your platform from [Releases](https://github.com/radiosilence/codeowners-lsp/releases).

### Zed

Use the [codeowners-zed](https://github.com/radiosilence/codeowners-zed) extension which automatically downloads and manages the LSP.

### Manual

```bash
# Add to PATH or configure your editor to use the binary
codeowners-lsp
```

The LSP communicates over stdio.

## Configuration

The LSP automatically finds CODEOWNERS files in standard locations:

- `.github/CODEOWNERS`
- `CODEOWNERS`
- `docs/CODEOWNERS`

### Initialization Options

```json
{
  "path": "custom/CODEOWNERS",
  "individual": "@username",
  "team": "@org/team-name"
}
```

- `path`: Custom CODEOWNERS file location (relative to workspace root)
- `individual`: Your personal GitHub handle for "take ownership as individual" actions
- `team`: Your team's GitHub handle for "take ownership as team" actions

## License

MIT
