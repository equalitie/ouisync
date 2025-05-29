package org.equalitie.ouisync.dart.example

import android.os.Bundle
import android.util.Log
import io.flutter.embedding.android.FlutterActivity

private const val TAG = "ouisync.example"

class MainActivity: FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        Log.d(TAG, "onCreate")
    }

    override fun onStart() {
        super.onStart()

        Log.d(TAG, "onStart")
    }

    override fun onRestart()  {
        super.onRestart()

        Log.d(TAG, "onRestart")
    }

    override fun onResume()  {
        super.onResume()

        Log.d(TAG, "onResume")
    }

    override fun onPause()  {
        super.onPause()

        Log.d(TAG, "onPause isFinishing=$isFinishing")
    }

    override fun onStop()  {
        super.onStop()

        Log.d(TAG, "onStop isFinishing=$isFinishing")
    }

    override fun onDestroy() {
        super.onDestroy()

        Log.d(TAG, "onDestroy isFinishing=$isFinishing")
    }

}
