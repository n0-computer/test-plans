package main

import (
	"fmt"

	bsmsg "github.com/ipfs/go-bitswap/message"
	peer "github.com/libp2p/go-libp2p/core/peer"
	"github.com/testground/sdk-go/runtime"
)

type TestplanTracer struct {
	runenv    *runtime.RunEnv
	recvStats *BitSwapMessageStats
	sendStats *BitSwapMessageStats
}

type BitSwapMessageStats struct {
	Count     int
	Size      int
	Blocks    int
	Haves     int
	DontHaves int
	Wants     int
	Cancels   int
}

func (s *BitSwapMessageStats) append(bms BitSwapMessageStats) {
	s.Count += bms.Count
	s.Size += bms.Size
	s.Blocks += bms.Blocks
	s.Haves += bms.Haves
	s.DontHaves += bms.DontHaves
	s.Wants += bms.Wants
	s.Cancels += bms.Cancels
}

func NewTracer(runenv *runtime.RunEnv, recvStats *BitSwapMessageStats, sendStats *BitSwapMessageStats) *TestplanTracer {
	return &TestplanTracer{
		runenv,
		recvStats,
		sendStats,
	}
}

func (t *TestplanTracer) processMsg(msg bsmsg.BitSwapMessage) BitSwapMessageStats {
	stats := BitSwapMessageStats{
		Count:     1,
		Size:      msg.Size(),
		Blocks:    len(msg.Blocks()),
		Haves:     len(msg.Haves()),
		DontHaves: len(msg.DontHaves()),
		Wants:     len(msg.Wantlist()),
		Cancels:   0,
	}

	for _, entry := range msg.Wantlist() {
		if entry.Cancel {
			stats.Cancels += 1
		}
	}

	t.runenv.RecordMessage(fmt.Sprintf("%+v", stats))
	return stats
}

func (t *TestplanTracer) MessageReceived(pid peer.ID, msg bsmsg.BitSwapMessage) {
	t.runenv.RecordMessage("MessageReceived")
	s := t.processMsg(msg)
	t.recvStats.append(s)
}

func (t *TestplanTracer) MessageSent(pid peer.ID, msg bsmsg.BitSwapMessage) {
	t.runenv.RecordMessage("MessageSent")
	s := t.processMsg(msg)
	t.sendStats.append(s)
}
