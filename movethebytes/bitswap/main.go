package main

import (
	"context"
	"errors"
	"fmt"
	"math/rand"
	"time"

	"github.com/testground/sdk-go/network"
	"github.com/testground/sdk-go/run"
	"github.com/testground/sdk-go/runtime"
	"github.com/testground/sdk-go/sync"

	"github.com/dustin/go-humanize"
	bitswap "github.com/ipfs/go-bitswap"
	bsnet "github.com/ipfs/go-bitswap/network"
	block "github.com/ipfs/go-block-format"
	"github.com/ipfs/go-cid"
	datastore "github.com/ipfs/go-datastore"
	blockstore "github.com/ipfs/go-ipfs-blockstore"
	exchange "github.com/ipfs/go-ipfs-exchange-interface"
	bstats "github.com/ipfs/go-ipfs-regression/bitswap"
	"github.com/libp2p/go-libp2p"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/multiformats/go-multiaddr"
	"github.com/multiformats/go-multihash"
)

var (
	testcases = map[string]interface{}{
		"speed-test": run.InitializedTestCaseFn(runSpeedTest),
	}
	networkState  = sync.State("network-configured")
	readyState    = sync.State("ready-to-publish")
	readyDLState  = sync.State("ready-to-download")
	doneState     = sync.State("done")
	providerTopic = sync.NewTopic("provider", &peer.AddrInfo{})
	blockTopic    = sync.NewTopic("blocks", &multihash.Multihash{})
)

func main() {
	run.InvokeMap(testcases)
}

func runSpeedTest(runenv *runtime.RunEnv, initCtx *run.InitContext) error {
	runenv.RecordMessage("running speed-test")
	client := initCtx.SyncClient
	ctx := context.Background()

	latStr := runenv.StringParam("latency")
	latency, err := time.ParseDuration(latStr)
	if err != nil {
		return err
	}
	bwStr := runenv.StringParam("bandwidth")
	bandwidth, err := humanize.ParseBytes(bwStr)
	if err != nil {
		return err
	}

	linkShape := network.LinkShape{
		Latency:   latency,
		Bandwidth: bandwidth * 8,
		Jitter:    0,
		// Filter: (not implemented)
		Loss:          0,
		Corrupt:       0,
		CorruptCorr:   0,
		Reorder:       0,
		ReorderCorr:   0,
		Duplicate:     0,
		DuplicateCorr: 0,
	}
	runenv.RecordMessage("latency is set at %v", latency)
	runenv.RecordMessage("bandwidth is set at %v", humanize.Bytes(bandwidth))

	runenv.RecordMessage("configuring")
	initCtx.NetClient.MustConfigureNetwork(ctx, &network.Config{
		Network:       "default",
		Enable:        true,
		Default:       linkShape,
		CallbackState: networkState,
		// CallbackTarget: runenv.TestGroupInstanceCount,
		RoutingPolicy: network.AllowAll,
	})
	seq := client.MustSignalAndWait(ctx, "ip-allocation", runenv.TestInstanceCount)
	runenv.RecordMessage("network configured")
	listen, err := multiaddr.NewMultiaddr(fmt.Sprintf("/ip4/%s/tcp/3333", initCtx.NetClient.MustGetDataNetworkIP().String()))
	if err != nil {
		return err
	}
	runenv.RecordMessage("listen configured")
	h, err := libp2p.New(libp2p.ListenAddrs(listen))
	if err != nil {
		return err
	}
	defer h.Close()
	runenv.RecordMessage("libp2p configured")
	kad, err := dht.New(ctx, h)
	if err != nil {
		return err
	}
	runenv.RecordMessage("dht configured")
	for _, a := range h.Addrs() {
		runenv.RecordMessage("listening on addr: %s", a.String())
	}
	runenv.RecordMessage("I am %d, peerID %v", seq, h.ID())
	recvStats := &BitSwapMessageStats{}
	sendStats := &BitSwapMessageStats{}
	tracer := NewTracer(runenv, recvStats, sendStats)
	bstore := blockstore.NewBlockstore(datastore.NewMapDatastore())
	ex := bitswap.New(ctx, bsnet.NewFromIpfsHost(h, kad), bstore, bitswap.WithTracer(tracer))
	switch runenv.TestGroupID {
	case "providers":
		runenv.RecordMessage("running provider")
		err = runProvide(ctx, runenv, h, bstore, ex, recvStats, sendStats)
	case "requestors":
		runenv.RecordMessage("running requestor")
		err = runRequest(ctx, runenv, h, bstore, ex, recvStats, sendStats)
	default:
		runenv.RecordMessage("not part of a group")
		err = errors.New("unknown test group id")
	}
	return err
}

func runProvide(ctx context.Context, runenv *runtime.RunEnv, h host.Host, bstore blockstore.Blockstore, ex exchange.Interface, recvStats *BitSwapMessageStats, sendStats *BitSwapMessageStats) error {
	tgc := sync.MustBoundClient(ctx, runenv)
	ai := peer.AddrInfo{
		ID:    h.ID(),
		Addrs: h.Addrs(),
	}
	tgc.MustPublish(ctx, providerTopic, &ai)
	tgc.MustSignalAndWait(ctx, readyState, runenv.TestInstanceCount)

	size := runenv.SizeParam("size")
	count := runenv.IntParam("count")
	for i := 0; i <= count; i++ {
		runenv.RecordMessage("generating %d-sized random block", size)
		buf := make([]byte, size)
		rand.Read(buf)
		blk := block.NewBlock(buf)
		err := bstore.Put(ctx, blk)
		if err != nil {
			return err
		}
		mh := blk.Multihash()
		runenv.RecordMessage("publishing block %s", mh.String())
		tgc.MustPublish(ctx, blockTopic, &mh)
	}

	tgc.MustSignalAndWait(ctx, readyDLState, runenv.TestInstanceCount)
	tgc.MustSignalAndWait(ctx, doneState, runenv.TestInstanceCount)
	runenv.RecordMessage(fmt.Sprintf("provider recv stats: %+v", recvStats))
	runenv.RecordMessage(fmt.Sprintf("provider send stats: %+v", sendStats))
	return nil
}

func runRequest(ctx context.Context, runenv *runtime.RunEnv, h host.Host, bstore blockstore.Blockstore, ex exchange.Interface, recvStats *BitSwapMessageStats, sendStats *BitSwapMessageStats) error {
	tgc := sync.MustBoundClient(ctx, runenv)
	providers := make(chan *peer.AddrInfo)
	blkmhs := make(chan *multihash.Multihash)
	providerSub, err := tgc.Subscribe(ctx, providerTopic, providers)
	if err != nil {
		return err
	}

	for i := 0; i < (runenv.TestInstanceCount - runenv.TestGroupInstanceCount); i++ {
		ai := <-providers

		runenv.RecordMessage("connecting to provider: %s", fmt.Sprint(*ai))

		err = h.Connect(ctx, *ai)
		if err != nil {
			return fmt.Errorf("could not connect to provider: %w", err)
		}

		runenv.RecordMessage("connected to provider")
	}

	providerSub.Done()

	blockmhSub, err := tgc.Subscribe(ctx, blockTopic, blkmhs)
	if err != nil {
		return fmt.Errorf("could not subscribe to block sub: %w", err)
	}
	defer blockmhSub.Done()

	// tell the provider that we're ready for it to publish blocks
	tgc.MustSignalAndWait(ctx, readyState, runenv.TestInstanceCount)
	// wait until the provider is ready for us to start downloading
	tgc.MustSignalAndWait(ctx, readyDLState, runenv.TestInstanceCount)

	begin := time.Now()
	count := runenv.IntParam("count")
	for i := 0; i < count; i++ {
		mh := <-blkmhs
		runenv.RecordMessage("downloading block %s", mh.String())
		dlBegin := time.Now()
		blk, err := ex.GetBlock(ctx, cid.NewCidV0(*mh))
		if err != nil {
			return fmt.Errorf("could not download get block %s: %w", mh.String(), err)
		}
		dlDuration := time.Since(dlBegin)
		s := &bstats.BitswapStat{
			SingleDownloadSpeed: &bstats.SingleDownloadSpeed{
				Cid:              blk.Cid().String(),
				DownloadDuration: dlDuration,
			},
		}
		runenv.RecordMessage(bstats.Marshal(s))
	}
	duration := time.Since(begin)
	s := &bstats.BitswapStat{
		MultipleDownloadSpeed: &bstats.MultipleDownloadSpeed{
			BlockCount:    count,
			TotalDuration: duration,
		},
	}
	runenv.RecordMessage(bstats.Marshal(s))
	runenv.RecordMessage(fmt.Sprintf("requestor recv stats: %+v", recvStats))
	runenv.RecordMessage(fmt.Sprintf("requestor send stats: %+v", sendStats))
	// tgc.MustSignalEntry(ctx, doneState) // TODO(arqu): this hangs the job completion
	tgc.MustSignalAndWait(ctx, doneState, runenv.TestInstanceCount)
	return nil
}
