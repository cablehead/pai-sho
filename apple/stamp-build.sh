#!/bin/sh
# The build number is the count of commits on pai-sho's history, stamped into
# the built app, so every new build that ships from a new commit is higher
# than the last, as TestFlight needs, without anyone bumping it by hand.
set -eu
n=$(git -C "$PROJECT_DIR" rev-list --count HEAD)
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $n" "$TARGET_BUILD_DIR/$INFOPLIST_PATH"
