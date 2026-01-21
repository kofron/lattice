#!/bin/bash
#
# Architecture lint for Lattice
#
# Per ARCHITECTURE.md Section 5 and Milestone 0.1:
# - Commands must go through gating before execution
# - Two patterns are supported:
#   1. Simple commands: use run_gated() wrapper (no direct scan imports)
#   2. Complex commands: use check_requirements() pre-flight (may still call scan internally)
#
# This script enforces these constraints in CI.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

echo "Checking architecture constraints..."

ERRORS=0

# Build list of command files that use check_requirements (pre-flight gating pattern)
# These are allowed to call scan() directly because they've passed gating first
PREFLIGHT_GATED_FILES=""
for file in src/cli/commands/*.rs; do
    if grep -q "check_requirements" "$file" 2>/dev/null; then
        PREFLIGHT_GATED_FILES="$PREFLIGHT_GATED_FILES $(basename "$file")"
    fi
done

# Check 1: Commands that DON'T use check_requirements should not import scan directly
echo -n "  Checking for ungated scan imports... "
VIOLATIONS=""
for file in src/cli/commands/*.rs; do
    basename=$(basename "$file")
    # Skip mod.rs (dispatcher), test files, and files using pre-flight gating
    if [ "$basename" = "mod.rs" ]; then
        continue
    fi
    if echo "$PREFLIGHT_GATED_FILES" | grep -q "$basename"; then
        continue
    fi
    # Check if this file imports scan without using run_gated
    if grep -q "use crate::engine::scan::scan" "$file" 2>/dev/null; then
        if ! grep -q "run_gated\|run_command" "$file" 2>/dev/null; then
            VIOLATIONS="$VIOLATIONS $file"
        fi
    fi
done

if [ -n "$VIOLATIONS" ]; then
    echo -e "${RED}FAIL${NC}"
    echo "ERROR: These files import scan() without gating:"
    echo "  $VIOLATIONS"
    echo "Use run_gated() wrapper or check_requirements() pre-flight"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}OK${NC}"
fi

# Check 2: All command files must use SOME form of gating
echo -n "  Checking all commands use gating... "
UNGATED=""
# Commands that are special cases (no repo required, or are the gating system itself)
EXEMPT="auth mod stack_comment_ops phase3_helpers changelog completion"

for file in src/cli/commands/*.rs; do
    basename=$(basename "$file" .rs)
    # Skip exempt files
    if echo "$EXEMPT" | grep -q "$basename"; then
        continue
    fi
    # Check for any form of gating
    if ! grep -q "run_gated\|run_command\|check_requirements" "$file" 2>/dev/null; then
        UNGATED="$UNGATED $basename"
    fi
done

if [ -n "$UNGATED" ]; then
    echo -e "${RED}FAIL${NC}"
    echo "ERROR: These commands have no gating:"
    echo "  $UNGATED"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}OK${NC}"
fi

# Check 3: Pre-flight gated files MUST call check_requirements before first scan
echo -n "  Checking pre-flight gating order... "
ORDER_VIOLATIONS=""
for file in src/cli/commands/*.rs; do
    if grep -q "check_requirements" "$file" 2>/dev/null; then
        # Get line numbers
        CHECK_LINE=$(grep -n "check_requirements" "$file" | head -1 | cut -d: -f1)
        SCAN_LINE=$(grep -n "scan(&git)" "$file" | head -1 | cut -d: -f1)
        if [ -n "$SCAN_LINE" ] && [ -n "$CHECK_LINE" ]; then
            if [ "$SCAN_LINE" -lt "$CHECK_LINE" ]; then
                ORDER_VIOLATIONS="$ORDER_VIOLATIONS $(basename "$file")"
            fi
        fi
    fi
done

if [ -n "$ORDER_VIOLATIONS" ]; then
    echo -e "${RED}FAIL${NC}"
    echo "ERROR: These files call scan() before check_requirements():"
    echo "  $ORDER_VIOLATIONS"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}OK${NC}"
fi

# Report summary of gating patterns used
echo ""
echo "Gating pattern summary:"
RUN_GATED_COUNT=$(grep -rl "run_gated" src/cli/commands/*.rs 2>/dev/null | wc -l | tr -d ' ')
PREFLIGHT_COUNT=$(grep -rl "check_requirements" src/cli/commands/*.rs 2>/dev/null | wc -l | tr -d ' ')
echo "  - run_gated() wrapper: $RUN_GATED_COUNT files"
echo "  - check_requirements() pre-flight: $PREFLIGHT_COUNT files"

echo ""
if [ $ERRORS -eq 0 ]; then
    echo -e "${GREEN}All architecture checks passed!${NC}"
    exit 0
else
    echo -e "${RED}$ERRORS architecture violation(s) found${NC}"
    exit 1
fi
