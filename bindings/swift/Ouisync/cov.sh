#!/usr/bin/env zsh
ENABLE_LOGGING=1 INCLUDE_SLOW=1 swift test --enable-code-coverage || (echo "One or more tests failed" && exit 1)

if [ "$1" = "report" ]; then
    xcrun llvm-cov report .build/debug/OuisyncPackageTests.xctest/Contents/MacOS/OuisyncPackageTests -instr-profile .build/debug/codecov/default.profdata --sources Sources/
else
    xcrun llvm-cov show .build/debug/OuisyncPackageTests.xctest/Contents/MacOS/OuisyncPackageTests -instr-profile .build/debug/codecov/default.profdata --sources Sources/ --format html > .build/codecov.html
    test "$1" != "open" || open .build/codecov.html
fi

