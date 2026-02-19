 Project: MassKritical
 working name-mesh
 Kraftbox 2026
 Billy Kiseu 2026

 Android-Desktop app for peer-to-peer mesh communication without internet, 
 for use in disaster recovery, organisation, or any situation where traditional networks are unavailable or untrusted. 
 Built with Rust and egui.

 Documentation in readme.md
 and quick build in build.md


1.  bare minimum, build, send and recieve text ---------done
2.  expand features: peer discover, file sharing, Voice notes, calling, beetr UI for the desktop -----done
3.  final set of major features, clean up system, validate, bluetooth, featire enhancemensts ar below-------progress

#Phase 3 Things to do 
 High value, realistic scope:                                                                                                                                                              - Bluetooth transport -- mesh works without WiFi, two phones in a field                                                                                                                 
  - Message persistence -- right now messages disappear when you close the app, storing chat history locally                                                                              
  - Read receipts / delivery confirmation -- know if your message actually reached the peer                                                                                               
  - Group chats -- named channels that multiple peers can join, not just broadcast-to-all or 1-to-1
  - Offline message queue -- if a peer is temporarily disconnected, hold messages and deliver when they reconnect

  UI/UX improvements:
  - Contact list / nicknames -- save peers you've seen before so they're recognized when they reappear
  - Notification sounds -- audio alert on incoming message, call, or SOS
  - Image preview -- if a received file is a photo, show a thumbnail inline in chat instead of just a filename
  - Typing indicator -- shows when someone is composing a message to you
  - Dark/light theme toggle on Android (desktop already has dark theme)

  Network resilience:
  - Store-and-forward relaying -- node C holds a message for offline node B until B comes back
  - Multi-hop routing improvements -- smarter routing than pure flooding (maybe gossip protocol or routing tables)
  - WiFi Direct / hotspot auto-creation -- if no shared network exists, one device creates a hotspot automatically

  Security:
  - Message signing verification -- verify messages actually came from who they claim (you have Ed25519 keys but messages aren't signed yet)
  - Disappearing messages -- auto-delete after a timer
  - Key fingerprint verification -- QR code or emoji-based safety number like Signal


i would wanna mask the app as if ts for disaster recovery, so that we dnt run into any legal issues
rebrand the app, with a logos and color scheme. 

maybe a couple of features that are like medical aid/disater area focued to hide behind 