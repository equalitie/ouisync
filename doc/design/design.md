% OuiSync
% eQualitie
% September 2020

# Introduction

The OuiSync project aims to provide users with a friendly tool to synchronize folders with files between their own devices or those of other users (either trusted or untrusted), while keeping the data private and secure both in transit and in rest, and all that in spite of poor or spotty connectivity between devices and without the need of dedicated servers or even local network infrastructure.

OuiSync implements a distributed protocol for the exchange and synchronization of file changes that allows it to use automatic merge strategies and simple conflict resolution propagation, thus sparing users from manual  or low-level handling of synchronization issues.

Communications between devices in OuiSync are resistant to interference thanks to technologies developed for the [Ouinet][] project. Peer-to-peer (P2P) techniques are used for the discovery of other devices and communication with them without the need of a server. Local discovery allows devices to talk directly over a (maybe isolated) local network. If no network is available at all, Wi-Fi Direct or Bluetooth can be used instead for direct device-to-device (D2D) communication.

[Ouinet]: https://github.com/equalitie/ouinet/

TODO: encryption

# Content

Content.
