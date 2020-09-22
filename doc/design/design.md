% OuiSync
% eQualitie
% September 2020

# Introduction

The OuiSync project aims to provide users with a friendly tool to synchronize folders with files between their own devices or those of other users (either trusted or untrusted), while keeping the data private and secure both in transit and in rest, and all that in spite of poor or spotty connectivity between devices and without the need of dedicated servers or even local network infrastructure.

OuiSync implements a distributed protocol for the exchange and synchronization of file changes that allows it to use automatic merge strategies and simple conflict resolution propagation, thus sparing users from manual or low-level handling of synchronization issues.

OuiSync uses strong encryption when communicating with other devices to protect from eavesdropping and tampering; data is also only propagated to devices selected for storage to enhance privacy, and then only an allowed subset can access the actual files and metadata. When storing data permanently, OuiSync also uses strong encryption that protects it in case of device seizure.

Communications between devices in OuiSync are resistant to interference thanks to technologies developed for the [Ouinet][] project. Peer-to-peer (P2P) techniques are used for the discovery of other devices and communication with them without the need of a server. Local discovery allows devices to talk directly over a (maybe isolated) local network. If no network is available at all, Wi-Fi Direct or Bluetooth can be used instead for direct device-to-device (D2D) communication.

[Ouinet]: https://github.com/equalitie/ouinet/

# Requirements

OuiSync caters to users who want to keep, for availability or backup purposes, copies of a set of files synchronized between different devices be them their own, or belonging to other users.

Also, some of these devices are trusted to access, modify and share file data (like an encrypted smartphone or desktop computer), while others (like a permanently online Raspberry Pi or virtual private server) are only trusted to blindly store and convey data to others.

Moreover, users want the system to behave in a way which is respectful with their privacy, secure, and available despite limited network connectivity.

The previous requirements are in contrast with the majority of existing solutions, where some of the following issues arise:

  - Users rely on third-party providers running servers which can access file data and become a bottleneck and single point of failure or control of the system.
  - Other devices need good network reachability (e.g. static IP address, port-forwarding router) or have to resort to dedicated rendez-vous or tunneling helper servers.
  - Devices need an active network connection even if they are physically close to each other.
  - All devices keeping copies of the data have access to their content.
  - Users need to adopt ad hoc workarounds (like encrypted file system layers or archive files), which break the usability of the system, to keep data private.
  - Conflicting modifications are handled in a user-unfriendly way (if supported at all).
  - Services, protocols and tools are proprietary or closed source, thus an inherent security liability.

In addition, OuiSync strives to fulfill its requirements in a user-friendly and accessible way, providing end-user tools ready to be used in the main mobile and desktop platforms. All protocols are open and software is released under Free/Libre and Open Source Software licenses.

# Usage scenarios

## Backup device

In this case, Alice has an encrypted smartphone that she uses to read work documents and take pictures of her journeys. She travels a lot and she worries that she might lose the phone and thus the files in it, but she does not want to reveal her files to untrusted third parties. Sitting in a drawer at home she also has an old Raspberry Pi that she used to watch videos on her TV.

So she uses OuiSync to create two *folders* in her phone: one with *Documents* and another with *Voyage pictures*. She also connects the Pi (whose SD card has much unused space) permanently to the router, installs OuiSync and creates one *safe* for each folder in the phone. The Pi has no storage encryption but it is not a risk to Alice since OuiSync safes only see encrypted data and have no access to file data nor metadata.

Now every time that Alice goes online with her phone, modifications to the folders (like added pictures) are exchanged peer-to-peer as encrypted data with the Pi and stored locally. OuiSync at the Pi keeps some history of changes, so Alice can safely remove old pictures from the phone or recover accidentally modified files.

TODO: diagram

## File sharing

Alice wants to share her *Voyage pictures* folders with Bob so that he can see the pictures and add more that he took with his camera when traveling together. So Bob installs OuiSync in his computer and configures a folder to synchronize with Alice's. Bob's device retrieves encrypted data both from Alice's phone and Pi peer-to-peer, so he's able to decrypt and access files in the folder after a while. When he adds new files, encrypted data is retrieved by the other devices as well, so the files become available in Alice's phone.

Since Alice's Pi is online all the time, it does not matter that Alice's and Bob's devices are not simultaneously online. Changes received by the Pi when either one of the devices is online end up being propagated to the other one when it gets connected.

TODO: diagram

## Storage incentives

The connection of Alice's home Pi is not specially robust, and now that Bob is adding his very high-quality camera pictures to the shared folder, it becomes quite clear that the Pi will soon run out of storage. So Alice and Bob start looking for bigger, more reliable alternative backup storage.

Charlie offers himself to host a OuiSync safe for them in one of his virtual servers in exchange for a small monthly amount. The servers have reliable and fast connections with plenty of storage space, so Alice and Bob accept the offer and together with Charlie they configure the new safe. When it comes online, it automatically starts gathering encrypted data from Alice's and Bob's devices, so it ends up with a copy of all data (but it still has no access to the files themselves).

Should Alice and Bob decide that they no longer want to use Charlie's services, they only need to find an alternative place to setup a new safe and let it replicate all of the folder's data before removing Charlie's safe.

TODO: diagram

## Conflict resolution

# Content

Content.
