import 'dart:async';
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:ouisync/internal/length_delimited_codec.dart';
import 'package:test/test.dart';

void main() {
  test('sanity check', () async {
    await _testCase(
      [utf8.encode('hello'), utf8.encode('world')],
      [1, 5, 10],
    );
  });

  test('one chunk', () async {
    await _testCase(
        _randomMessages(
          Random(),
          count: 100,
          minLength: 0,
          maxLength: 54 * 1024,
        ),
        []);
  });

  test('random messages', () async {
    final random = Random();

    await _testCase(
      _randomMessages(
        random,
        count: 100,
        minLength: 0,
        maxLength: 64 * 1024,
      ),
      random.intsInRange(1, 32 * 1024),
    );
  });
}

Future<void> _testCase(
  List<Uint8List> messages,
  Iterable<int> chunkLengths,
) async {
  // Encode all messages into one big buffer.
  final builder = BytesBuilder();
  for (final message in messages) {
    builder.add(encodeLengthDelimited(message));
  }
  final buffer = builder.takeBytes();

  final controller = StreamController<Uint8List>();

  try {
    // Split the buffer into randomly sized chunks and send those chunks to the steam controller.
    int offset = 0;
    for (final chunkLength in chunkLengths) {
      if (offset >= buffer.length) {
        break;
      }

      final chunk = buffer.sublist(
          offset, offset + min(chunkLength, buffer.length - offset));

      controller.add(chunk);
      offset += chunk.length;
    }

    // Final chunk
    final chunk = buffer.sublist(offset, buffer.length);
    if (chunk.isNotEmpty) {
      controller.add(chunk);
    }

    // Receive decoded messages from the controller.
    final receivedMessages = await controller.stream
        .transform(LengthDelimitedCodec())
        .take(messages.length)
        .toList();

    expect(receivedMessages, equals(messages));
  } finally {
    await controller.close();
  }
}

List<Uint8List> _randomMessages(
  Random random, {
  required int count,
  required int minLength,
  required int maxLength,
}) =>
    List.generate(
      count,
      (_) => Uint8List.fromList(
        List.generate(
          random.nextIntInRange(minLength, maxLength),
          (_) => random.nextInt(256),
        ),
      ),
    );

extension _RandomExtension on Random {
  int nextIntInRange(int min, int max) => nextInt(max - min) + min;

  Iterable<int> intsInRange(int min, int max) sync* {
    while (true) {
      yield nextIntInRange(min, max);
    }
  }
}
