#!/usr/bin/env zsh
INCLUDE_SLOW=1 swift test --enable-code-coverage
if [ "$1" = "report" ]; then
    xcrun llvm-cov report .build/debug/OuisyncPackageTests.xctest/Contents/MacOS/OuisyncPackageTests -instr-profile .build/debug/codecov/default.profdata --sources Sources/
else
    xcrun llvm-cov show .build/debug/OuisyncPackageTests.xctest/Contents/MacOS/OuisyncPackageTests -instr-profile .build/debug/codecov/default.profdata --sources Sources/ --format html > .build/codecov.html
    test "$1" != "open" || open .build/codecov.html
fi

