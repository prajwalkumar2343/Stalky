# Taste (Continuously Learned by [prajwalkumar2343][cmd])

[cmd]: https://commandcode.ai/

# git-workflow
- Commit in a way to maximize number of commits - prefer smaller, more frequent commits over large ones. Confidence: 0.90
- Use specific branch naming: `feature/*` for features, `fix/*` for bug fixes, `temp/*` for agent/temporary work, `onboarding/*` for onboarding flows. Confidence: 0.85
- Push to feature branches first, create PRs rather than direct pushes to main. Confidence: 0.80
- Rebase branches when they fall behind main. Confidence: 0.75

# code-quality
- Code quality should be top notch - assume someone from the creation team will review the code. Confidence: 0.90
- Use skills/MCP tools (like code-review-graph) for code review before committing. Confidence: 0.80
- Run tests and verify builds pass before pushing to GitHub. Confidence: 0.75

# implementation-approach
- Make detailed plans first before implementing - break work into small, specific tasks. Confidence: 0.85
- Implement one task at a time, verify it works before moving to next. Confidence: 0.75
- For multi-agent work, split into clearly separated parts to avoid interference. Confidence: 0.70

# project-structure
- Prefer modular architecture with clear separation of concerns. Confidence: 0.80
- Keep code organized in dedicated folders by functionality. Confidence: 0.75
- Design for plugin-based extensibility where appropriate. Confidence: 0.70
