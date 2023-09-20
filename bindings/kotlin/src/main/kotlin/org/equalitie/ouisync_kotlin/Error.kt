package org.equalitie.ouisync_kotlin

import org.equalitie.ouisync_kotlin.ErrorCode

class Error(code: ErrorCode, message: String) : Exception(message)
